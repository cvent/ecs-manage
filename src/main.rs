#[macro_use]
extern crate clap;
#[macro_use]
extern crate structopt;
extern crate rusoto_core;
extern crate rusoto_credential;
extern crate rusoto_ecr;
extern crate rusoto_ecs;
extern crate rusoto_elbv2;
#[macro_use]
extern crate failure;
extern crate backoff;
#[macro_use]
extern crate log;
extern crate stderrlog;
#[macro_use]
extern crate maplit;
extern crate itertools;
extern crate serde;
extern crate serde_json;

mod args;
mod helpers;
mod services;

use failure::Error;
use serde_json::Number as JsonNumber;
use serde_json::Value;
use serde_json::Value::Number;
use std::collections::HashMap;
use std::thread;
use std::time::Duration;
use structopt::StructOpt;

use args::Args;
use args::EcsCommand::*;
use args::ServiceProperty;
use args::ServicesCommand::*;

fn main() -> Result<(), Error> {
    let args = Args::from_args();

    stderrlog::new()
        .module(module_path!())
        .verbosity(args.verbosity + 2)
        .init()?;

    trace!("Args: {:?}", args);

    match args.command {
        ServicesCommand {
            command: Info { cluster, region },
        } => {
            let ecs_client = helpers::ecs_client(args.profile, region)?;
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
            command: Audit { cluster, region },
        } => {
            let ecs_client = helpers::ecs_client(args.profile.clone(), region.clone())?;
            let ecr_client = helpers::ecr_client(args.profile.clone(), region.clone())?;
            let elb_client = helpers::elb_client(args.profile, region)?;
            for service in services::describe_services(&ecs_client, cluster)? {
                let service_name = services::service_name(&service)?;

                let audit_message =
                    services::audit_service(&ecs_client, &ecr_client, &elb_client, &service)?
                        .join(", ");

                if !audit_message.is_empty() {
                    println!("{} [{}]", service_name, audit_message);
                }
            }
        }
        ServicesCommand {
            command:
                Compare {
                    source_cluster,
                    source_region,
                    destination_cluster,
                    destination_region,
                },
        } => {
            let destination_ecs_client =
                helpers::ecs_client(args.profile.clone(), destination_region)?;
            let source_ecs_client = helpers::ecs_client(args.profile.clone(), source_region)?;
            let source_only_services = services::compare_services(
                &source_ecs_client,
                source_cluster.clone(),
                &destination_ecs_client,
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
                    source_region,
                    destination_cluster,
                    destination_region,
                    role_suffix,
                },
        } => {
            let destination_ecs_client =
                helpers::ecs_client(args.profile.clone(), destination_region)?;
            let source_ecs_client =
                helpers::ecs_client(args.profile.clone(), source_region.clone())?;
            let source_ecr_client =
                helpers::ecr_client(args.profile.clone(), source_region.clone())?;
            let source_elb_client = helpers::elb_client(args.profile, source_region.clone())?;
            let source_only_services = services::compare_services(
                &source_ecs_client,
                source_cluster.clone(),
                &destination_ecs_client,
                destination_cluster.clone(),
            )?;

            for source_service in source_only_services {
                if services::audit_service(
                    &source_ecs_client,
                    &source_ecr_client,
                    &source_elb_client,
                    &source_service,
                )?
                .is_empty()
                {
                    thread::sleep(Duration::from_millis(10000));

                    services::create_service(
                        &destination_ecs_client,
                        destination_cluster.clone(),
                        source_service,
                        role_suffix.clone(),
                    )?;
                }
            }
        }
        ServicesCommand {
            command:
                Export {
                    cluster,
                    region,
                    property,
                },
        } => {
            let ecs_client = helpers::ecs_client(args.profile, region)?;

            let service_properties = services::describe_services(&ecs_client, cluster.clone())?
                .into_iter()
                .map(|s| {
                    let property_value = match property {
                        ServiceProperty::DesiredCount => s.desired_count,
                    };

                    Ok((
                        services::service_name(&s)?,
                        Number(JsonNumber::from(property_value.unwrap())),
                    ))
                })
                .collect::<Result<HashMap<String, Value>, Error>>()?;

            println!("{}", serde_json::to_string_pretty(&service_properties)?);
        }
        ServicesCommand {
            command:
                Update {
                    cluster,
                    region,
                    modification,
                    sleep,
                },
        } => {
            let ecs_client = helpers::ecs_client(args.profile, region)?;
            for service in services::describe_services(&ecs_client, cluster.clone())? {
                services::update_service(
                    &ecs_client,
                    cluster.clone(),
                    service.clone(),
                    modification.clone(),
                )?;

                thread::sleep(Duration::from_millis(sleep));
            }
        }
    }

    Ok(())
}
