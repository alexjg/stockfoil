use super::*;
use subduction_tokio::{Error, error::ErrorKind};

#[test]
fn shutdown_reports_must_be_graceful_and_complete() {
    let graceful = CloseReport {
        forced: false,
        unfinished_tasks: 0,
        failure: None,
    };
    check_close(graceful.clone()).unwrap();
    let forced = check_close(CloseReport {
        forced: true,
        ..graceful.clone()
    })
    .unwrap_err();
    assert!(forced.to_string().contains("forced=true"));
    let unfinished = check_close(CloseReport {
        unfinished_tasks: 1,
        ..graceful.clone()
    })
    .unwrap_err();
    assert!(unfinished.to_string().contains("unfinished_tasks=1"));
    let failure = check_close(CloseReport {
        failure: Some(Error::new(ErrorKind::Background, "core task failed")),
        ..graceful
    })
    .unwrap_err();
    assert_eq!(
        failure.downcast_ref::<Error>().unwrap().kind(),
        ErrorKind::Background
    );
}

#[test]
fn cleanup_failure_is_reported_without_losing_the_operation_error() {
    let failure = with_cleanup(
        Err::<(), _>(std::io::Error::other("original failure").into()),
        Err(eyre::eyre!("shutdown failure")),
    )
    .unwrap_err();
    assert!(failure.downcast_ref::<std::io::Error>().is_some());
    let message = format!("{failure:#}");
    assert!(message.contains("original failure"));
    assert!(message.contains("shutdown failure"));
    assert!(with_cleanup(Ok(()), Err(eyre::eyre!("shutdown failure"))).is_err());
    assert!(with_cleanup(Err::<(), _>(eyre::eyre!("operation failure")), Ok(())).is_err());
    assert_eq!(with_cleanup(Ok(42), Ok(())).unwrap(), 42);
}
