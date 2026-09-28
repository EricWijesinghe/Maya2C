//! ROS 2 message serialization: OMG CDR (XCDR1), little-endian, as ROS 2's
//! default middleware writes it. A message is a 4-byte encapsulation header
//! (`00 01 00 00` for little-endian CDR) and then its fields, each aligned to
//! its own size relative to the end of that header; a string is a `u32`
//! length that counts its terminating NUL, the bytes, and the NUL.
//!
//! Two messages a swarm needs: `geometry_msgs/Pose` and `PoseStamped`.

use crate::SwarmError;

/// Little-endian plain CDR.
const CDR_LE: [u8; 4] = [0x00, 0x01, 0x00, 0x00];
/// Longest frame id accepted.
pub const MAX_STRING: usize = 256;

/// `geometry_msgs/Pose`: position, then orientation as a quaternion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// x, y, z (m).
    pub position: [f64; 3],
    /// x, y, z, w.
    pub orientation: [f64; 4],
}

/// `geometry_msgs/PoseStamped`.
#[derive(Clone, Debug, PartialEq)]
pub struct PoseStamped {
    /// Seconds.
    pub sec: i32,
    /// Nanoseconds.
    pub nanosec: u32,
    /// Coordinate frame.
    pub frame_id: String,
    /// The pose.
    pub pose: Pose,
}

struct Writer(Vec<u8>);

impl Writer {
    fn align(&mut self, n: usize) {
        while !(self.0.len() - CDR_LE.len()).is_multiple_of(n) {
            self.0.push(0);
        }
    }
    fn f64(&mut self, v: f64) {
        self.align(8);
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.align(4);
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn string(&mut self, s: &str) {
        self.u32(u32::try_from(s.len() + 1).unwrap_or(u32::MAX));
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
    }
    fn pose(&mut self, p: &Pose) {
        p.position
            .iter()
            .chain(&p.orientation)
            .for_each(|v| self.f64(*v));
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn align(&mut self, n: usize) {
        let rel = self.at - CDR_LE.len();
        self.at += (n - rel % n) % n;
    }
    fn take<const N: usize>(&mut self) -> Result<[u8; N], SwarmError> {
        let out = self
            .bytes
            .get(self.at..self.at + N)
            .ok_or(SwarmError::Malformed("CDR truncated"))?;
        self.at += N;
        out.try_into().map_err(|_| SwarmError::Malformed("CDR"))
    }
    fn f64(&mut self) -> Result<f64, SwarmError> {
        self.align(8);
        Ok(f64::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> Result<u32, SwarmError> {
        self.align(4);
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn string(&mut self) -> Result<String, SwarmError> {
        let n = usize::try_from(self.u32()?).map_err(|_| SwarmError::Malformed("CDR string"))?;
        if n == 0 || n > MAX_STRING + 1 {
            return Err(SwarmError::Malformed("CDR string length"));
        }
        let raw = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or(SwarmError::Malformed("CDR truncated"))?;
        self.at += n;
        let (text, nul) = raw.split_at(n - 1);
        if nul != [0] {
            return Err(SwarmError::Malformed("CDR string without its NUL"));
        }
        String::from_utf8(text.to_vec())
            .map_err(|_| SwarmError::Malformed("CDR string is not UTF-8"))
    }
    fn pose(&mut self) -> Result<Pose, SwarmError> {
        let mut v = [0.0; 7];
        for slot in &mut v {
            *slot = self.f64()?;
        }
        Ok(Pose {
            position: [v[0], v[1], v[2]],
            orientation: [v[3], v[4], v[5], v[6]],
        })
    }
    fn start(bytes: &[u8]) -> Result<Reader<'_>, SwarmError> {
        if bytes.get(..4) != Some(&CDR_LE[..]) {
            return Err(SwarmError::Unsupported(
                "CDR encapsulation other than little-endian plain CDR",
            ));
        }
        Ok(Reader {
            bytes,
            at: CDR_LE.len(),
        })
    }
    fn finish(&self) -> Result<(), SwarmError> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(SwarmError::Malformed("CDR trailing bytes"))
        }
    }
}

/// Serializes a `Pose`.
#[must_use]
pub fn encode_pose(p: &Pose) -> Vec<u8> {
    let mut w = Writer(CDR_LE.to_vec());
    w.pose(p);
    w.0
}

/// Deserializes a `Pose`.
///
/// # Errors
///
/// Another encapsulation, truncation, or trailing bytes.
pub fn decode_pose(bytes: &[u8]) -> Result<Pose, SwarmError> {
    let mut r = Reader::start(bytes)?;
    let pose = r.pose()?;
    r.finish()?;
    Ok(pose)
}

/// Serializes a `PoseStamped`.
#[must_use]
pub fn encode_pose_stamped(p: &PoseStamped) -> Vec<u8> {
    let mut w = Writer(CDR_LE.to_vec());
    w.0.extend_from_slice(&p.sec.to_le_bytes());
    w.u32(p.nanosec);
    w.string(&p.frame_id);
    w.pose(&p.pose);
    w.0
}

/// Deserializes a `PoseStamped`.
///
/// # Errors
///
/// As [`decode_pose`], and a malformed frame id.
pub fn decode_pose_stamped(bytes: &[u8]) -> Result<PoseStamped, SwarmError> {
    let mut r = Reader::start(bytes)?;
    let sec = i32::from_le_bytes(r.u32()?.to_le_bytes());
    let nanosec = r.u32()?;
    let frame_id = r.string()?;
    let pose = r.pose()?;
    r.finish()?;
    Ok(PoseStamped {
        sec,
        nanosec,
        frame_id,
        pose,
    })
}
