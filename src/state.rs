//! Shared bounded state serialization for CLAP and VST3.

use crate::params::GainSnapParams;

/// Four-byte GainSnap state marker (`GNSP`).
pub const STATE_MAGIC: u32 = u32::from_le_bytes(*b"GNSP");
/// State envelope version written by the toggle matcher.
pub const STATE_VERSION: u32 = 3;
/// State envelope version written by the original one-shot matcher.
pub const LEGACY_STATE_VERSION: u32 = 1;
/// State envelope versions accepted for loading.
pub const ACCEPTED_STATE_VERSIONS: &[u32] = &[STATE_VERSION, 2, LEGACY_STATE_VERSION];

/// Fixed state payload size in bytes.
pub const STATE_PAYLOAD_BYTES: usize = 12;

/// Decodeable GainSnap state snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StateSnapshot {
    /// Selected target level in dBFS.
    pub target_db: f32,
    /// Whether match measurement was enabled when saved.
    pub match_requested: bool,
    /// Last calculated gain correction in dB.
    pub locked_gain_db: f32,
    /// RMS mode; older projects default to Peak.
    pub rms_mode: bool,
}

/// Encode the current parameters as a fixed-size little-endian payload.
pub fn encode_payload(params: &GainSnapParams) -> [u8; STATE_PAYLOAD_BYTES] {
    let mut payload = [0_u8; STATE_PAYLOAD_BYTES];
    payload[0..4].copy_from_slice(&params.target_db().to_le_bytes());
    payload[4] = u8::from(params.match_requested());
    payload[5] = u8::from(params.rms_mode());
    payload[8..12].copy_from_slice(&params.locked_gain_db().to_le_bytes());
    payload
}

/// Decode and validate a fixed-size state payload for a supported version.
///
/// Version one stored a high Match Now value as a one-shot trigger. It must
/// not reopen a continuous measurement when loaded by the toggle matcher, so
/// legacy payloads always migrate to Match off while preserving their target
/// and last locked gain.
pub fn decode_payload(version: u32, payload: &[u8]) -> Option<StateSnapshot> {
    if !ACCEPTED_STATE_VERSIONS.contains(&version) {
        return None;
    }
    if payload.len() != STATE_PAYLOAD_BYTES
        || payload[6..8] != [0, 0]
        || payload[5] > u8::from(version == STATE_VERSION)
    {
        return None;
    }
    let target_db = f32::from_le_bytes(payload[0..4].try_into().ok()?);
    let locked_gain_db = f32::from_le_bytes(payload[8..12].try_into().ok()?);
    if !target_db.is_finite() || !locked_gain_db.is_finite() || payload[4] > 1 {
        return None;
    }
    Some(StateSnapshot {
        target_db,
        match_requested: version != LEGACY_STATE_VERSION && payload[4] != 0,
        rms_mode: version == STATE_VERSION && payload[5] != 0,
        locked_gain_db,
    })
}

/// Apply a validated snapshot to the shared atomic store.
pub fn apply_snapshot(params: &GainSnapParams, snapshot: StateSnapshot) {
    params.set_param(crate::params::PARAM_RMS_MODE, f32::from(snapshot.rms_mode));
    params.set_param(crate::params::PARAM_TARGET_DB, snapshot.target_db);
    params.set_param(
        crate::params::PARAM_MATCH,
        f32::from(snapshot.match_requested),
    );
    params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, snapshot.locked_gain_db);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_mode_round_trips_and_v2_projects_retain_peak_mode_and_matching() {
        let params = GainSnapParams::new();
        params.set_param(crate::params::PARAM_MATCH, 1.0);
        let old = encode_payload(&params);
        params.set_param(crate::params::PARAM_RMS_MODE, 1.0);
        let saved = encode_payload(&params);
        let snapshot = decode_payload(STATE_VERSION, &saved).unwrap();
        assert!(snapshot.rms_mode);
        let restored = GainSnapParams::new();
        apply_snapshot(&restored, snapshot);
        assert!(restored.rms_mode() && restored.match_requested());
        assert!(decode_payload(2, &saved).is_none());
        apply_snapshot(&restored, decode_payload(2, &old).unwrap());
        assert!(!restored.rms_mode() && restored.match_requested());
    }

    #[test]
    fn state_round_trips_and_rejects_malformed_payloads() {
        let params = GainSnapParams::new();
        params.set_param(crate::params::PARAM_TARGET_DB, -7.5);
        params.set_param(crate::params::PARAM_MATCH, 1.0);
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 5.25);
        let payload = encode_payload(&params);
        let decoded = decode_payload(STATE_VERSION, &payload).expect("valid state");
        assert_eq!(decoded.target_db, -7.5);
        assert!(decoded.match_requested);
        assert_eq!(decoded.locked_gain_db, 5.25);

        assert!(decode_payload(STATE_VERSION, &payload[..11]).is_none());
        let mut invalid = payload;
        invalid[4] = 2;
        assert!(decode_payload(STATE_VERSION, &invalid).is_none());
        invalid = payload;
        invalid[5] = 2;
        assert!(decode_payload(STATE_VERSION, &invalid).is_none());
    }

    #[test]
    fn legacy_state_migrates_match_now_to_off() {
        let params = GainSnapParams::new();
        params.set_param(crate::params::PARAM_TARGET_DB, -7.5);
        params.set_param(crate::params::PARAM_MATCH, 1.0);
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 5.25);
        let payload = encode_payload(&params);
        let decoded = decode_payload(LEGACY_STATE_VERSION, &payload).expect("legacy state");

        assert_eq!(decoded.target_db, -7.5);
        assert!(!decoded.match_requested);
        assert_eq!(decoded.locked_gain_db, 5.25);
    }
}
