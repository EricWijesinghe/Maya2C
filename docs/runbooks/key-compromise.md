# Runbook: Validator or operator key compromise

**Symptom.** Signatures you did not make; unexpected transfers.

**Check.** Signer logs; slashing-protection DB; where the keystore lived.

**Act.** Rotate: stop the signer, move stake or funds with a fresh key, revoke the old one where the account type allows (smart-account rotation). Report per docs/security/INCIDENT_RESPONSE.md.

**Rehearsal.** not rehearsed
