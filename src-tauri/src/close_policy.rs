#[derive(Debug, PartialEq)]
pub(crate) enum Decision {
    Allow,
    Confirm,
    Wait,
    Save,
    Hide,
}
/// `to_tray`: a tray icon exists and the user did not choose Quit; jobs keep running hidden.
pub(crate) fn decide(
    jobs: usize,
    confirmed: bool,
    saving: bool,
    ready: bool,
    to_tray: bool,
) -> Decision {
    if ready {
        Decision::Allow
    } else if to_tray && !saving {
        Decision::Hide
    } else if jobs > 0 && !confirmed {
        Decision::Confirm
    } else if saving {
        Decision::Wait
    } else {
        Decision::Save
    }
}
pub(crate) fn should_finish(
    saving: bool,
    ready: bool,
    acknowledged: bool,
    elapsed_ms: u64,
) -> bool {
    saving && !ready && (acknowledged || elapsed_ms >= 2000)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_save_wait_and_timeout() {
        assert_eq!(decide(1, false, false, false, false), Decision::Confirm);
        assert_eq!(decide(0, false, false, false, false), Decision::Save);
        assert_eq!(decide(1, true, false, false, false), Decision::Save);
        assert_eq!(decide(0, false, true, false, false), Decision::Wait);
        assert_eq!(decide(1, false, true, true, false), Decision::Allow);
        assert_eq!(decide(1, false, false, false, true), Decision::Hide);
        assert_eq!(decide(0, false, true, false, true), Decision::Wait);
        assert_eq!(decide(0, false, false, true, true), Decision::Allow);
        assert!(!should_finish(true, false, false, 1999));
        assert!(should_finish(true, false, false, 2000));
        assert!(should_finish(true, false, true, 0));
        assert!(!should_finish(false, false, true, 0));
        assert!(!should_finish(true, true, true, 2000));
    }
}
