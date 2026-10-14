//! The optional config file: every setting has a default, bad files don't crash.

use tapin::config::{Config, NotifyOn};

#[test]
fn no_file_means_defaults() {
    let c = Config::parse("").unwrap();
    assert!(c.notify);
    assert_eq!(c.notify_on, vec![NotifyOn::Needs, NotifyOn::Done]);
    assert!(!c.bell);
    assert_eq!(c.approval_timeout_secs, 300);
    assert_eq!(c.codex_approval_timeout_secs, 30);
}

#[test]
fn settings_override_defaults() {
    let c = Config::parse("notify = false\nbell = true\napproval_timeout_secs = 30\n").unwrap();
    assert!(!c.notify && c.bell);
    assert_eq!(c.approval_timeout_secs, 30);
}

#[test]
fn unknown_keys_and_bad_values_are_errors_with_the_reason() {
    let err = Config::parse("notfy = false\n").unwrap_err().to_string();
    assert!(err.contains("notfy"), "{err}");
    assert!(Config::parse("notify_on = [\"sometimes\"]\n").is_err());
}
