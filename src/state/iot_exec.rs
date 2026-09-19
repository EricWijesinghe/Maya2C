//! Executing the IoT anchor transactions.
//!
//! ## Who bears a failure
//!
//! - **Owner transactions** — enrollment, revocation — are the sender's own, so
//!   a bad proof, a second enrollment, or a revocation by someone else is an
//!   error, as an HTLC lock that cannot be made is.
//! - **Device messages** are relayed by any gateway. A bad signature is the
//!   relay's error, but everything that merely *loses* — an unknown device, a
//!   duplicate relay, a batch overtaken by a newer one, a device already
//!   terminal — is a no-op, because two gateways racing is ordinary and an
//!   error would void the block the winner is in (invariant 7).
//! - **Clone evidence** conflicting with a recorded batch is detected inside
//!   `SubmitTelemetry` itself, so a copied key is caught even if nobody files a
//!   `ProveEquivocation`.
//!
//! Tamper reports, equivocation proofs and revocations belong to no breaker
//! module: halting them while a key is being abused would protect the abuser.

use maya_iot_anchor::rules::{BatchOutcome, record_batch};
use maya_iot_anchor::{
    DeviceId, DeviceRecord, DeviceStatus, Enrollment, Equivocation, Progress, TamperEvent,
    TelemetryBatch, judge_batch,
};

use crate::core::TxKind;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};

impl StateDB {
    /// Records a device against its owner.
    ///
    /// # Errors
    ///
    /// [`NodeError::Iot`] before activation, for a proof that does not bind this
    /// owner, or for a device already enrolled.
    pub(crate) fn enroll_device(
        &self,
        overlay: &mut Overlay,
        owner: &Address,
        enrollment: &Enrollment,
        context: BlockContext,
    ) -> Result<()> {
        require_active(context)?;
        enrollment
            .verify(owner)
            .map_err(|e| refused(format!("enrollment: {e}")))?;
        let device = enrollment.device();
        if self.iot_device(overlay, &device)?.is_some() {
            return Err(refused(format!(
                "device {} is already enrolled",
                hex::encode(device)
            )));
        }
        let record = DeviceRecord {
            owner: *owner,
            public_key: enrollment.public_key,
            class: enrollment.class,
            bounds: enrollment.bounds,
            status: DeviceStatus::Active,
            enrolled_height: context.height,
            progress: Progress::default(),
        };
        StateDB::put_iot_device(overlay, &record);
        Ok(())
    }

    /// Judges a relayed batch; see the module docs for what is a no-op.
    ///
    /// # Errors
    ///
    /// [`NodeError::Iot`] before activation or for a signature that does not
    /// verify under the enrolled key.
    pub(crate) fn submit_telemetry(
        &self,
        overlay: &mut Overlay,
        batch: &TelemetryBatch,
        context: BlockContext,
    ) -> Result<()> {
        require_active(context)?;
        let Some(mut record) = self.iot_device(overlay, &batch.device)? else {
            return Ok(());
        };
        batch
            .verify(&record.public_key)
            .map_err(|e| refused(format!("telemetry: {e}")))?;
        let view = batch.view();
        match judge_batch(record.status, &record.progress, &record.bounds, &view) {
            BatchOutcome::Recorded { anomalous } => {
                record.progress = record_batch(&record.progress, &view, anomalous, context.height);
            }
            BatchOutcome::Equivocation => record.status = DeviceStatus::Compromised,
            BatchOutcome::Duplicate
            | BatchOutcome::Stale
            | BatchOutcome::Unlinked
            | BatchOutcome::Inactive => {
                return Ok(());
            }
        }
        StateDB::put_iot_device(overlay, &record);
        Ok(())
    }

    /// Marks a device tampered on its own signed report.
    ///
    /// # Errors
    ///
    /// [`NodeError::Iot`] before activation, for an unknown device, or a bad
    /// signature.
    pub(crate) fn report_tamper(
        &self,
        overlay: &mut Overlay,
        event: &TamperEvent,
        context: BlockContext,
    ) -> Result<()> {
        require_active(context)?;
        let mut record = self.require_device(overlay, &event.device)?;
        event
            .verify(&record.public_key)
            .map_err(|e| refused(format!("tamper event: {e}")))?;
        terminate(overlay, &mut record, DeviceStatus::Tampered);
        Ok(())
    }

    /// Marks a device compromised on two conflicting batches.
    ///
    /// # Errors
    ///
    /// [`NodeError::Iot`] before activation, for an unknown device, or for
    /// batches that do not verify or do not conflict.
    pub(crate) fn prove_equivocation(
        &self,
        overlay: &mut Overlay,
        evidence: &Equivocation,
        context: BlockContext,
    ) -> Result<()> {
        require_active(context)?;
        let mut record = self.require_device(overlay, &evidence.first.device)?;
        evidence
            .check(&record.public_key)
            .map_err(|e| refused(format!("equivocation: {e}")))?;
        terminate(overlay, &mut record, DeviceStatus::Compromised);
        Ok(())
    }

    /// Revokes a device at its owner's request.
    ///
    /// # Errors
    ///
    /// [`NodeError::Iot`] before activation, for an unknown device, or a sender
    /// that is not the owner.
    pub(crate) fn revoke_device(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        device: &DeviceId,
        context: BlockContext,
    ) -> Result<()> {
        require_active(context)?;
        let mut record = self.require_device(overlay, device)?;
        if record.owner != *sender {
            return Err(refused(format!(
                "{} does not own device {}",
                hex::encode(sender),
                hex::encode(device)
            )));
        }
        terminate(overlay, &mut record, DeviceStatus::Revoked);
        Ok(())
    }

    fn require_device(&self, overlay: &Overlay, device: &DeviceId) -> Result<DeviceRecord> {
        self.iot_device(overlay, device)?
            .ok_or_else(|| refused(format!("unknown device {}", hex::encode(device))))
    }
}

/// Moves an active device to `status`. A device already terminal stays as it
/// is: the first terminal state is the record of what happened.
fn terminate(overlay: &mut Overlay, record: &mut DeviceRecord, status: DeviceStatus) {
    if record.status.is_terminal() {
        return;
    }
    record.status = status;
    StateDB::put_iot_device(overlay, record);
}

/// Mempool admission for IoT transactions: every signature checked before the
/// pool holds a byte of it, against the committed key where one is needed.
///
/// # Errors
///
/// [`NodeError::Iot`] for a proof or signature that does not verify, or a
/// device message naming no enrolled device.
pub fn admit(state: &StateDB, sender: &Address, kind: &TxKind) -> Result<()> {
    let device_key = |device: &DeviceId| {
        state
            .stored_iot_device(device)?
            .map(|record| record.public_key)
            .ok_or_else(|| refused(format!("unknown device {}", hex::encode(device))))
    };
    let checked = match kind {
        TxKind::EnrollDevice(enrollment) => enrollment.verify(sender),
        TxKind::SubmitTelemetry(batch) => batch.verify(&device_key(&batch.device)?),
        TxKind::ReportTamper(event) => event.verify(&device_key(&event.device)?),
        TxKind::ProveEquivocation(evidence) => evidence.check(&device_key(&evidence.first.device)?),
        _ => return Ok(()),
    };
    checked.map_err(|e| refused(e.to_string()))
}

fn refused(reason: impl Into<String>) -> NodeError {
    NodeError::Iot(reason.into())
}

fn require_active(context: BlockContext) -> Result<()> {
    if context.iot_active() {
        Ok(())
    } else {
        Err(refused(format!(
            "the iot anchor is not active at height {}",
            context.height
        )))
    }
}
