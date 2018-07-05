#[macro_use]
extern crate structopt;
extern crate rusoto_core;
extern crate rusoto_ecr;
extern crate rusoto_ecs;
extern crate rusoto_elbv2;
#[macro_use]
extern crate failure;
extern crate backoff;
extern crate tokio_core;
#[macro_use]
extern crate log;
extern crate loggerv;
#[macro_use]
extern crate maplit;
extern crate itertools;

mod args;
mod helpers;
mod services;

use failure::Error;
use loggerv::Logger;
use rusoto_core::reactor::RequestDispatcher;
use rusoto_core::{ChainProvider, ProfileProvider};
use rusoto_ecr::EcrClient;
use rusoto_ecs::EcsClient;
use rusoto_elbv2::ElbClient;
use structopt::StructOpt;
use tokio_core::reactor::Core;
use std::time::Duration;
use std::thread;

use args::Args;
use args::EcsCommand::*;
use args::ServicesCommand::*;

fn main() -> Result<(), Error> {
    let args = Args::from_args();

    Logger::new()
        .verbosity(args.verbosity)
        .level(true)
        .module_path(true)
        .init()?;

    let credentials_provider = {
        let core = Core::new()?;
        match args.profile {
            Some(profile) => ChainProvider::with_profile_provider(&core.handle(), {
                let mut p = ProfileProvider::new()?;
                p.set_profile(profile);
                p
            }),
            None => ChainProvider::new(&core.handle()),
        }
    };

    let ecs_client = EcsClient::new(
        RequestDispatcher::default(),
        credentials_provider.clone(),
        args.region.parse()?,
    );

    let ecr_client = EcrClient::new(
        RequestDispatcher::default(),
        credentials_provider.clone(),
        args.region.parse()?,
    );

    let elb_client = ElbClient::new(
        RequestDispatcher::default(),
        credentials_provider.clone(),
        args.region.parse()?,
    );

    match args.command {
        ServicesCommand {
            command: Info { cluster },
        } => {
            for service in services::describe_services(&ecs_client, cluster.clone())? {
                let service_name = services::service_name(&service)?;

                println!(
                    "{}/{} - Task: {} - Desired Count: {}",
                    cluster,
                    service_name,
                    service.task_definition.ok_or(format_err!(
                        "Service {:?} has no task definition",
                        &service_name
                    ))?,
                    service
                        .desired_count
                        .ok_or(format_err!("Service {} has no desired count", service_name))?,
                );
            }
        }
        ServicesCommand {
            command: Audit { cluster },
        } => for service in services::describe_services(&ecs_client, cluster)? {
            let service_name = services::service_name(&service)?;

            let audit_message =
                services::audit_service(&ecs_client, &ecr_client, &elb_client, &service)?
                    .join(", ");

            if !audit_message.is_empty() {
                println!("{} [{}]", service_name, audit_message);
            }
        },
        ServicesCommand {
            command:
                Compare {
                    source_cluster,
                    destination_cluster,
                },
        } => {
            let source_only_services = services::compare_services(
                &ecs_client,
                source_cluster.clone(),
                destination_cluster,
            )?;

            println!("Not in destination:");
            for service in &source_only_services {
                println!("{}/{}", source_cluster, services::service_name(&service)?);
            }

            println!("Total: {}", source_only_services.len());
        }
        ServicesCommand {
            command:
                Sync {
                    source_cluster,
                    destination_cluster,
                    role_suffix,
                },
        } => {
            let source_only_services = services::compare_services(
                &ecs_client,
                source_cluster.clone(),
                destination_cluster.clone(),
            )?;

            for source_service in source_only_services {
                if services::audit_service(&ecs_client, &ecr_client, &elb_client, &source_service)?
                    .is_empty()
                {
                    thread::sleep(Duration::from_millis(4000))
                    services::create_service(
                        &ecs_client,
                        destination_cluster.clone(),
                        source_service,
                        role_suffix.clone(),
                    )?;
                }
            }
        }
        ServicesCommand {
            command: Update {
                cluster,
                modification,
            },
        } => for service in services::describe_services(&ecs_client, cluster.clone())? {
            services::update_service(
                &ecs_client,
                cluster.clone(),
                service.clone(),
                modification.clone(),
            )?;
        },
    }

    Ok(())
}
