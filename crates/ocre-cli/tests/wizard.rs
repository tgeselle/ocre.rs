use std::io::{Error, ErrorKind};

use super::*;

#[test]
fn interrupted_prompts_cancel_and_other_errors_pass_through() {
    assert_eq!(prompt_error(Error::from(ErrorKind::Interrupted)).message, "cancelled");
    let err = prompt_error(Error::new(ErrorKind::BrokenPipe, "terminal closed"));
    assert_eq!((err.message.as_str(), err.hint), ("terminal closed", None));
}
