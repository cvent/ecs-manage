use backoff::{self, ExponentialBackoff, Operation};
use failure::Error;
use rusoto_core::request::HttpClient;
use rusoto_core::Region;
use rusoto_credential::{ChainProvider, ProfileProvider};
use rusoto_ecr::EcrClient;
use rusoto_ecs::EcsClient;
use rusoto_elbv2::ElbClient;

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

pub fn credentials_provider(profile: Option<String>) -> Result<ChainProvider, Error> {
    match profile {
        Some(profile) => Ok(ChainProvider::with_profile_provider({
            let mut p = ProfileProvider::new()?;
            p.set_profile(profile);
            p
        })),
        None => Ok(ChainProvider::new()),
    }
}

pub fn ecs_client(profile: Option<String>, region: Region) -> Result<EcsClient, Error> {
    Ok(EcsClient::new_with(
        HttpClient::new()?,
        credentials_provider(profile)?,
        region,
    ))
}

pub fn elb_client(profile: Option<String>, region: Region) -> Result<ElbClient, Error> {
    Ok(ElbClient::new_with(
        HttpClient::new()?,
        credentials_provider(profile)?,
        region,
    ))
}

pub fn ecr_client(profile: Option<String>, region: Region) -> Result<EcrClient, Error> {
    Ok(EcrClient::new_with(
        HttpClient::new()?,
        credentials_provider(profile)?,
        region,
    ))
}
