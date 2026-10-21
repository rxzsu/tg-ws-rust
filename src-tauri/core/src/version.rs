//! Version comparison for the update checker.
//!
//! The old check compared tags for inequality, so a locally newer build
//! (or a pre-release tag) reported a "new version" that was actually older.
//! Tags are compared numerically, segment by segment; a leading `v` and any
//! pre-release suffix are ignored for ordering purposes.

fn parse_segments(version: &str) -> Vec<u64> {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        // Stop at the first pre-release/build marker: 1.2.3-beta+1 → 1.2.3.
        .split(['-', '+'])
        .next()
        .unwrap_or_default()
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u64>()
                .unwrap_or(0)
        })
        .collect()
}

/// True when `latest` is strictly newer than `current` (semver-ish).
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    let latest = parse_segments(latest);
    let current = parse_segments(current);
    if latest.iter().all(|p| *p == 0) {
        // Empty/garbage tag: never advertise an update.
        return false;
    }
    for i in 0..latest.len().max(current.len()) {
        let a = latest.get(i).copied().unwrap_or(0);
        let b = current.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_numerically() {
        assert!(is_newer_version("v1.0.1", "1.0.0"));
        assert!(is_newer_version("1.0.10", "1.0.9"));
        assert!(is_newer_version("1.1", "1.0.9"));
        assert!(!is_newer_version("v1.0.0", "1.0.0"));
        assert!(!is_newer_version("v0.9.9", "1.0.0"));
        assert!(!is_newer_version("", "1.0.0"));
        assert!(!is_newer_version("v", "1.0.0"));
        assert!(is_newer_version("v2.0.0-beta.1", "1.9.9"));
    }
}
