use super::*;
#[test]
fn typed_filesystem_kinds_are_not_inferred_from_display_or_redacted_text() {
    for (error, expected) in [
        (FsError::ProtectedWrite, ToolErrorKind::NeedsApproval),
        (FsError::Unapproved, ToolErrorKind::NeedsApproval),
        (FsError::HardDenied, ToolErrorKind::DeniedCapability),
        (FsError::Symlink, ToolErrorKind::DeniedCapability),
        (FsError::NotFound, ToolErrorKind::NotFound),
        (FsError::InvalidUtf8, ToolErrorKind::InvalidArgs),
        (FsError::FileTooLarge, ToolErrorKind::OutputLimit),
        (FsError::InputTooLarge, ToolErrorKind::OutputLimit),
        (FsError::Changed, ToolErrorKind::Failed),
    ] {
        assert_eq!(Failure::fs(error).kind(), expected);
    }
}
