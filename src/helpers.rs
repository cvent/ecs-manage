use backoff::{self, ExponentialBackoff, Operation};

use std::fmt::Display;

pub fn retry_log<S, T, E, F>(msg: S, mut op: F) -> Result<T, backoff::Error<E>>
where
    S: Display,
    E: Display,
    F: FnMut() -> Result<T, backoff::Error<E>>,
{
    op.retry_notify(&mut ExponentialBackoff::default(), |err, _| {
        info!("{} failed due to {}. Retrying", msg, err);
    })
}
