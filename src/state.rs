//! Shared bounded state serialization for CLAP and VST3.

use crate::params::GainSnapParams;

/// Four-byte GainSnap state marker (`GNSP`).
pub const STATE_MAGIC: u32 = u32::from_le_bytes(*b"GNSP");
/// State envelope version written by the toggle matcher.
pub const STATE_VERSION: u32 = 5;
/// State envelope version written by the original one-shot matcher.
pub const LEGACY_STATE_VERSION: u32 = 1;
/// State envelope versions accepted for loading.
pub const ACCEPTED_STATE_VERSIONS: &[u32] = &[STATE_VERSION, 4, 3, 2, LEGACY_STATE_VERSION];

/// Fixed state payload size in bytes.
pub const STATE_PAYLOAD_BYTES: usize = 16;

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
    /// Whether a completed match arms held-level protection after restore.
    pub has_match_result: bool,
    /// User-adjustable gain and mode; absent in states before V5.
    pub manual_gain_db: f32,
    pub manual_mode: bool,
}

/// Encode the current parameters as a fixed-size little-endian payload.
pub fn encode_payload(params: &GainSnapParams) -> [u8; STATE_PAYLOAD_BYTES] {
    let mut payload = [0_u8; STATE_PAYLOAD_BYTES];
    payload[0..4].copy_from_slice(&params.target_db().to_le_bytes());
    payload[4] = u8::from(params.match_requested());
    payload[5] = u8::from(params.rms_mode());
    payload[6] = u8::from(params.has_match_result());
    payload[7] = u8::from(params.manual_mode());
    payload[8..12].copy_from_slice(&params.locked_gain_db().to_le_bytes());
    payload[12..16].copy_from_slice(&params.manual_gain_db().to_le_bytes());
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
    let expected_len = if version >= 5 {
        STATE_PAYLOAD_BYTES
    } else {
        12
    };
    if payload.len() != expected_len
        || payload[7] > u8::from(version >= 5)
        || payload[6] > u8::from(version >= 4)
        || payload[5] > u8::from(version >= 3)
    {
        return None;
    }
    let target_db = f32::from_le_bytes(payload[0..4].try_into().ok()?);
    let locked_gain_db = f32::from_le_bytes(payload[8..12].try_into().ok()?);
    let manual_gain_db = if version >= 5 {
        f32::from_le_bytes(payload[12..16].try_into().ok()?)
    } else {
        0.0
    };
    if !target_db.is_finite()
        || !locked_gain_db.is_finite()
        || !manual_gain_db.is_finite()
        || payload[4] > 1
    {
        return None;
    }
    Some(StateSnapshot {
        target_db,
        match_requested: version != LEGACY_STATE_VERSION && payload[4] != 0,
        rms_mode: version >= 3 && payload[5] != 0,
        locked_gain_db,
        has_match_result: version >= 4 && payload[6] != 0,
        manual_gain_db,
        manual_mode: version >= 5 && payload[7] != 0,
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
    params.set_param(crate::params::PARAM_MANUAL_GAIN_DB, snapshot.manual_gain_db);
    params.set_param(
        crate::params::PARAM_MANUAL_MODE,
        f32::from(snapshot.manual_mode),
    );
    params.set_has_match_result(snapshot.has_match_result);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_mode_round_trips_and_older_projects_keep_their_mode() {
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
        let mut v3 = saved;
        v3[6] = 0;
        let old_rms = decode_payload(3, &v3[..12]).expect("valid version 3 state");
        assert!(old_rms.rms_mode);
        assert!(!old_rms.has_match_result);
        assert!(decode_payload(2, &saved).is_none());
        apply_snapshot(&restored, decode_payload(2, &old[..12]).unwrap());
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
        invalid = payload;
        invalid[6] = 2;
        assert!(decode_payload(STATE_VERSION, &invalid).is_none());
        invalid = payload;
        invalid[7] = 2;
        assert!(decode_payload(STATE_VERSION, &invalid).is_none());
    }

    #[test]
    fn legacy_state_migrates_match_now_to_off() {
        let params = GainSnapParams::new();
        params.set_param(crate::params::PARAM_TARGET_DB, -7.5);
        params.set_param(crate::params::PARAM_MATCH, 1.0);
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 5.25);
        let payload = encode_payload(&params);
        let decoded = decode_payload(LEGACY_STATE_VERSION, &payload[..12]).expect("legacy state");

        assert_eq!(decoded.target_db, -7.5);
        assert!(!decoded.match_requested);
        assert_eq!(decoded.locked_gain_db, 5.25);
    }

    #[test]
    fn high_peak_gain_round_trips_without_changing_state_format() {
        let params = GainSnapParams::new();
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 32.0);
        let payload = encode_payload(&params);

        let decoded = decode_payload(STATE_VERSION, &payload).expect("valid high-gain state");
        assert_eq!(decoded.locked_gain_db, 32.0);

        let restored = GainSnapParams::new();
        apply_snapshot(&restored, decoded);
        assert_eq!(restored.locked_gain_db(), 32.0);
    }

    #[test]
    fn completed_match_flag_round_trips_but_legacy_state_does_not_arm_it() {
        let params = GainSnapParams::new();
        params.set_has_match_result(true);
        let payload = encode_payload(&params);
        let decoded = decode_payload(STATE_VERSION, &payload).expect("valid version 5 state");
        assert!(decoded.has_match_result);

        let restored = GainSnapParams::new();
        apply_snapshot(&restored, decoded);
        assert!(restored.has_match_result());

        let mut previous = payload;
        previous[6] = 0;
        for version in [1, 2, 3] {
            let older = decode_payload(version, &previous[..12]).expect("valid older state");
            assert!(!older.has_match_result);
            apply_snapshot(&restored, older);
            assert!(!restored.has_match_result());
        }
    }

    #[test]
    fn manual_state_round_trips_and_v4_defaults_to_auto() {
        let params = GainSnapParams::new();
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, -9.0);
        params.set_param(crate::params::PARAM_MANUAL_GAIN_DB, 7.25);
        params.set_param(crate::params::PARAM_MANUAL_MODE, 1.0);
        params.set_has_match_result(true);
        let payload = encode_payload(&params);
        let restored = GainSnapParams::new();
        apply_snapshot(&restored, decode_payload(STATE_VERSION, &payload).unwrap());
        assert!(restored.manual_mode());
        assert_eq!(restored.manual_gain_db(), 7.25);
        assert!(restored.has_match_result());

        let mut v4 = payload;
        v4[7] = 0;
        apply_snapshot(&restored, decode_payload(4, &v4[..12]).unwrap());
        assert!(!restored.manual_mode());
        assert_eq!(restored.manual_gain_db(), 0.0);
        assert_eq!(restored.locked_gain_db(), -9.0);
        assert!(restored.has_match_result());
    }
}
