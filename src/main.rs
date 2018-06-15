#[macro_use]
extern crate structopt;
extern crate rusoto_core;
extern crate rusoto_ecs;
#[macro_use]
extern crate failure;
extern crate tokio_core;

use failure::Error;
use structopt::StructOpt;
use rusoto_core::reactor::RequestDispatcher;
use rusoto_core::{ChainProvider, ProfileProvider, ProvideAwsCredentials};
use rusoto_ecs::{Ecs, EcsClient, ListServicesRequest, DescribeServicesRequest, Service, CreateServiceRequest};
use tokio_core::reactor::Core;

use std::thread;
use std::time::Duration;

#[derive(Debug, StructOpt)]
#[structopt()]
struct Args {
    #[structopt(long = "profile")]
    profile: Option<String>,
    #[structopt(long = "region")]
    region: String,
    #[structopt(subcommand)]
    command: EcsCommand,
}

#[derive(Debug, StructOpt)]
enum EcsCommand {
    #[structopt(name = "info")]
    Info {
        cluster: String,
    },
    #[structopt(name = "compare")]
    Compare {
        source_cluster: String,
        destination_cluster: String,
    },
    #[structopt(name = "sync")]
    Sync {
        source_cluster: String,
        destination_cluster: String,
        role_suffix: Option<String>,
    },

}

fn main() -> Result<(), Error> {
    let args = Args::from_args();

    let core = Core::new()?;

    let credentials_provider = match args.profile {
        Some(profile) => ChainProvider::with_profile_provider(&core.handle(), {
            let mut p = ProfileProvider::new()?;
            p.set_profile(profile);
            p
        }),
        None => ChainProvider::new(&core.handle())
    };

    let client = EcsClient::new(
        RequestDispatcher::default(),
        credentials_provider,
        args.region.parse()?
    );

    match args.command {
        EcsCommand::Info { cluster } => {
            for description in describe_services(&client, cluster)? {
                println!("{:?} - Task: {:?} - Desired Count: {:?}", description.service_name, description.task_definition, description.desired_count);
            }
        },
        EcsCommand::Compare { source_cluster, destination_cluster } => {
            let source_services = describe_services(&client, source_cluster)?;
            pause();
            let destination_services = describe_services(&client, destination_cluster)?;

            let destination_names = destination_services.into_iter().map(|s| s.service_name).collect::<Vec<Option<String>>>();

            let source_only = source_services.into_iter().filter(|s| !destination_names.contains(&s.service_name)).collect::<Vec<Service>>();

            println!("Not in destination:");
            for service in &source_only {
                println!("{:?}", service.service_name);
            }

            println!("Total: {}", source_only.len());
        },
        EcsCommand::Sync { source_cluster, destination_cluster, role_suffix } => {
            let source_services = describe_services(&client, source_cluster)?;
            pause();
            let destination_services = describe_services(&client, destination_cluster.clone())?;

            let destination_names = destination_services.into_iter().map(|s| s.service_name).collect::<Vec<Option<String>>>();

            let source_only = source_services.into_iter().filter(|s| !destination_names.contains(&s.service_name));

            for source_service in source_only {
                println!("Creating {:?} in {}", source_service.service_name, destination_cluster);
                create_service(&client, destination_cluster.clone(), source_service, role_suffix.clone())?;
            }
        }
    }

    Ok(())
}

fn list_services<P: ProvideAwsCredentials + 'static>(client: &EcsClient<P, RequestDispatcher>, cluster: String) -> Result<Vec<String>, Error> {
    let mut token = Some(String::new());

    let mut services = Vec::new();

    while token.is_some() {
        let res = client.list_services(&ListServicesRequest {
            cluster: Some(cluster.clone()),
            launch_type: None,
            max_results: None,
            next_token: token,
        }).sync()?;

        if let Some(mut arns) = res.service_arns {
            services.append(&mut arns)
        };

        token = res.next_token;
    }

    Ok(services)
}

fn describe_service<P: ProvideAwsCredentials + 'static>(client: &EcsClient<P, RequestDispatcher>, cluster: String, service: String) -> Result<Service, Error> {
    let res = client.describe_services(&DescribeServicesRequest {
        cluster: Some(cluster),
        services: vec![service.clone()],
    }).sync()?;

    if let Some(failures) = res.failures {
        if !failures.is_empty() {
            bail!("Failures: {:?}", failures);
        }
    }

    match res.services {
        None => bail!("No service description for {}", service),
        Some(mut services) => Ok(services.pop().unwrap())
    }
}

fn describe_services<P: ProvideAwsCredentials + 'static>(client: &EcsClient<P, RequestDispatcher>, cluster: String) -> Result<Vec<Service>, Error> {
    list_services(&client, cluster.clone())?
        .into_iter()
        .map(|service| describe_service(&client, cluster.clone(), service))
        .collect()
}

fn create_service<P: ProvideAwsCredentials + 'static>(client: &EcsClient<P, RequestDispatcher>, cluster: String, from_service: Service, role_suffix: Option<String>) -> Result<Service, Error> {
    let role = if from_service.load_balancers.clone().map_or(false, |l| !l.is_empty()) {
        if from_service.network_configuration.clone().map_or(false, |n| n.awsvpc_configuration.is_some()) {
            // Handle awsvpc services specially
            Some(String::from("aws-service-role/ecs.amazonaws.com/AWSServiceRoleForECS"))
        } else {
            Some(format!("{}-{}", cluster, role_suffix.unwrap_or(String::from("ECSServiceRole"))))
        }
    } else {
        None
    };

    match from_service.clone().service_name {
        Some(service_name) => {
            let res = client.create_service(&CreateServiceRequest {
                client_token: None,
                cluster: Some(cluster),
                deployment_configuration: from_service.deployment_configuration,
                desired_count: from_service.desired_count.ok_or_else(|| format_err!("No desired count found for {:?}", service_name.clone()))?,
                health_check_grace_period_seconds: from_service.health_check_grace_period_seconds,
                launch_type: from_service.launch_type,
                load_balancers: from_service.load_balancers,
                network_configuration: from_service.network_configuration,
                placement_constraints: from_service.placement_constraints,
                placement_strategy: from_service.placement_strategy,
                platform_version: from_service.platform_version,
                role: role.clone(),
                service_name: service_name.clone(),
                task_definition: from_service.task_definition.ok_or_else(|| format_err!("No task definition found for {:?}", service_name.clone()))?,
            }).sync()?;

            pause();

            res.service.ok_or_else(|| format_err!("Tried to create service, but nothing returned"))
        },
        None => bail!("No service name found")
    }
}

fn pause() {
    thread::sleep(Duration::from_secs(5));
}
