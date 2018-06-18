#[macro_use]
extern crate structopt;
extern crate rusoto_core;
extern crate rusoto_ecr;
extern crate rusoto_ecs;
#[macro_use]
extern crate failure;
extern crate backoff;
extern crate tokio_core;
#[macro_use]
extern crate log;
extern crate loggerv;

use backoff::{ExponentialBackoff, Operation};
use failure::Error;
use loggerv::Logger;
use rusoto_core::reactor::RequestDispatcher;
use rusoto_core::{ChainProvider, ProfileProvider, ProvideAwsCredentials};
use rusoto_ecr::{DescribeImagesRequest, Ecr, EcrClient, ImageDetail, ImageIdentifier};
use rusoto_ecs::{
    CreateServiceRequest, DescribeServicesError, DescribeServicesRequest,
    DescribeTaskDefinitionError, DescribeTaskDefinitionRequest, Ecs, EcsClient, ListServicesError,
    ListServicesRequest, Service,
};
use structopt::StructOpt;
use tokio_core::reactor::Core;

use std::fmt::Display;

#[derive(Debug, StructOpt)]
struct Args {
    #[structopt(long = "profile")]
    profile: Option<String>,
    #[structopt(long = "region")]
    region: String,
    /// Sets the level of verbosity
    #[structopt(short = "v", long = "verbose", parse(from_occurrences), raw(global = "true"))]
    pub verbosity: u64,
    #[structopt(subcommand)]
    command: EcsCommand,
}

#[derive(Debug, StructOpt)]
enum EcsCommand {
    #[structopt(name = "info")]
    Info { cluster: String },
    #[structopt(name = "audit")]
    Audit { cluster: String },
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

    Logger::new()
        .verbosity(args.verbosity)
        .level(true)
        .module_path(true)
        .init()?;

    let core = Core::new()?;

    let credentials_provider = match args.profile {
        Some(profile) => ChainProvider::with_profile_provider(&core.handle(), {
            let mut p = ProfileProvider::new()?;
            p.set_profile(profile);
            p
        }),
        None => ChainProvider::new(&core.handle()),
    };

    let ecs_client = EcsClient::new(
        RequestDispatcher::default(),
        credentials_provider.clone(),
        args.region.parse()?,
    );

    let ecr_client = EcrClient::new(
        RequestDispatcher::default(),
        credentials_provider,
        args.region.parse()?,
    );

    match args.command {
        EcsCommand::Info { cluster } => {
            for description in describe_services(&ecs_client, cluster)? {
                println!(
                    "{:?} - Task: {:?} - Desired Count: {:?}",
                    description.service_name,
                    description.task_definition,
                    description.desired_count
                );
            }
        }
        EcsCommand::Audit { cluster } => {
            let no_ecr_images = describe_services(&ecs_client, cluster)?
                .into_iter()
                .filter(|s| {
                    ecr_images(&ecs_client, &ecr_client, s.clone())
                        .unwrap_or_default()
                        .iter()
                        .any(|image| image.is_err())
                });

            println!("Services without ECR images:");
            for service in no_ecr_images {
                if let Some(service_name) = service.service_name {
                    println!("{}", service_name);
                }
            }
        }
        EcsCommand::Compare {
            source_cluster,
            destination_cluster,
        } => {
            let source_services = describe_services(&ecs_client, source_cluster)?;
            let destination_services = describe_services(&ecs_client, destination_cluster)?;

            let destination_names = destination_services
                .into_iter()
                .map(|s| s.service_name)
                .collect::<Vec<Option<String>>>();

            let source_only = source_services
                .into_iter()
                .filter(|s| !destination_names.contains(&s.service_name))
                .collect::<Vec<Service>>();

            println!("Not in destination:");
            for service in &source_only {
                println!("{:?}", service.service_name);
            }

            println!("Total: {}", source_only.len());
        }
        EcsCommand::Sync {
            source_cluster,
            destination_cluster,
            role_suffix,
        } => {
            let source_services = describe_services(&ecs_client, source_cluster)?;
            let destination_services = describe_services(&ecs_client, destination_cluster.clone())?;

            let destination_names = destination_services
                .into_iter()
                .map(|s| s.service_name)
                .collect::<Vec<Option<String>>>();

            let source_only = source_services
                .into_iter()
                .filter(|s| !destination_names.contains(&s.service_name));

            for source_service in source_only {
                create_service(
                    &ecs_client,
                    destination_cluster.clone(),
                    source_service,
                    role_suffix.clone(),
                )?;
            }
        }
    }

    Ok(())
}

fn list_services<P: ProvideAwsCredentials + 'static>(
    client: &EcsClient<P, RequestDispatcher>,
    cluster: String,
) -> Result<Vec<String>, Error> {
    let mut token = Some(String::new());

    let mut services = Vec::new();

    while token.is_some() {
        let res = retry_log(format!("listing services in {}", cluster), || {
            client
                .list_services(&ListServicesRequest {
                    cluster: Some(cluster.clone()),
                    launch_type: None,
                    max_results: None,
                    next_token: token.clone(),
                })
                .sync()
                .map_err(|e| match e {
                    ListServicesError::Unknown(s) => {
                        if s == r#"{"__type":"ThrottlingException","message":"Rate exceeded"}"# {
                            backoff::Error::Transient(ListServicesError::Unknown(s))
                        } else {
                            backoff::Error::Permanent(ListServicesError::Unknown(s))
                        }
                    }
                    _ => backoff::Error::Permanent(e),
                })
        })?;
        if let Some(mut arns) = res.service_arns {
            services.append(&mut arns)
        };

        token = res.next_token;
    }

    Ok(services)
}

fn describe_service<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    cluster: String,
    service: String,
) -> Result<Service, Error> {
    let res = retry_log(format!("describing {}/{}", cluster, service), || {
        ecs_client
            .describe_services(&DescribeServicesRequest {
                cluster: Some(cluster.clone()),
                services: vec![service.clone()],
            })
            .sync()
            .map_err(|e| match e {
                DescribeServicesError::Unknown(s) => {
                    if s == r#"{"__type":"ThrottlingException","message":"Rate exceeded"}"# {
                        backoff::Error::Transient(DescribeServicesError::Unknown(s))
                    } else {
                        backoff::Error::Permanent(DescribeServicesError::Unknown(s))
                    }
                }
                _ => backoff::Error::Permanent(e),
            })
    })?;

    if let Some(failures) = res.failures {
        if !failures.is_empty() {
            bail!("Failures: {:?}", failures);
        }
    }

    match res.services {
        None => bail!("No service description for {}", service),
        Some(mut services) => Ok(services.pop().unwrap()),
    }
}

fn describe_services<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    cluster: String,
) -> Result<Vec<Service>, Error> {
    list_services(&ecs_client, cluster.clone())?
        .into_iter()
        .map(|service| describe_service(&ecs_client, cluster.clone(), service))
        .collect()
}

fn create_service<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    cluster: String,
    from_service: Service,
    role_suffix: Option<String>,
) -> Result<Service, Error> {
    let has_loadbalancer = from_service
        .load_balancers
        .clone()
        .map_or(false, |l| !l.is_empty());
    let is_awsvpc = from_service
        .network_configuration
        .clone()
        .map_or(false, |n| n.awsvpc_configuration.is_some());

    let role = if has_loadbalancer && !is_awsvpc {
        Some(format!(
            "{}-{}",
            cluster,
            role_suffix.unwrap_or(String::from("ECSServiceRole"))
        ))
    } else {
        None
    };

    match from_service.clone().service_name {
        Some(service_name) => {
            println!(
                "Creating {} in {} with the following role: {:?}",
                service_name, cluster, role
            );

            let res = ecs_client
                .create_service(&CreateServiceRequest {
                    client_token: None,
                    cluster: Some(cluster),
                    deployment_configuration: from_service.deployment_configuration,
                    desired_count: from_service.desired_count.ok_or_else(|| {
                        format_err!("No desired count found for {:?}", service_name.clone())
                    })?,
                    health_check_grace_period_seconds: from_service
                        .health_check_grace_period_seconds,
                    launch_type: from_service.launch_type,
                    load_balancers: from_service.load_balancers,
                    network_configuration: from_service.network_configuration,
                    placement_constraints: from_service.placement_constraints,
                    placement_strategy: from_service.placement_strategy,
                    platform_version: from_service.platform_version,
                    role: role.clone(),
                    service_name: service_name.clone(),
                    task_definition: from_service.task_definition.ok_or_else(|| {
                        format_err!("No task definition found for {:?}", service_name.clone())
                    })?,
                })
                .sync()?;

            res.service
                .ok_or_else(|| format_err!("Tried to create service, but nothing returned"))
        }
        None => bail!("No service name found"),
    }
}

fn ecr_images<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    ecr_client: &EcrClient<P, RequestDispatcher>,
    service: Service,
) -> Result<Vec<Result<ImageDetail, Error>>, Error> {
    match service.task_definition {
        Some(task_definition) => {
            let task_definition = retry_log(format!("describing {}", task_definition), || {
                ecs_client
                    .describe_task_definition(&DescribeTaskDefinitionRequest {
                        task_definition: task_definition.clone(),
                    })
                    .sync()
                    .map_err(|e| match e {
                        DescribeTaskDefinitionError::Unknown(s) => {
                            if s == r#"{"__type":"ThrottlingException","message":"Rate exceeded"}"# {
                                backoff::Error::Transient(DescribeTaskDefinitionError::Unknown(s))
                            } else {
                                backoff::Error::Permanent(DescribeTaskDefinitionError::Unknown(s))
                            }
                        }
                        _ => backoff::Error::Permanent(e),
                    })
            })?.task_definition;

            match task_definition {
                Some(task_definition) => match task_definition.container_definitions {
                    Some(cds) => {
                        let image_arns = cds
                            .into_iter()
                            .map(|cd| cd.image)
                            .collect::<Vec<Option<String>>>();

                        let mut images = Vec::new();

                        for image_arn in image_arns {
                            match image_arn {
                                Some(image_arn) => {
                                    match image_arn.split('/').collect::<Vec<&str>>().pop() {
                                        Some(repo_image) => {
                                            let split_repo_image =
                                                repo_image.split(':').collect::<Vec<&str>>();

                                            let image_id = ImageIdentifier {
                                                image_digest: None,
                                                image_tag: Some(split_repo_image[1].to_string()),
                                            };

                                            let mut image_details_res = ecr_client
                                                .describe_images(&DescribeImagesRequest {
                                                    filter: None,
                                                    image_ids: Some(vec![image_id]),
                                                    max_results: None,
                                                    next_token: None,
                                                    registry_id: None,
                                                    repository_name: split_repo_image[0]
                                                        .to_string(),
                                                })
                                                .sync();

                                            match image_details_res {
                                                Ok(image_details_res) => {
                                                    match image_details_res.image_details {
                                                        Some(mut image_details) => {
                                                            match image_details.pop() {
                                                                Some(image_detail) => {
                                                                    images.push(Ok(image_detail));
                                                                }
                                                                None => {}
                                                            }
                                                        }
                                                        None => {}
                                                    }
                                                }
                                                Err(e) => images.push(Err(e.into())),
                                            }
                                        }
                                        None => {}
                                    }
                                }
                                None => {}
                            }
                        }

                        Ok(images)
                    }
                    None => Ok(Vec::new()),
                },
                None => Ok(Vec::new()),
            }
        }
        None => Ok(Vec::new()),
    }
}

fn retry_log<S, T, E, F>(msg: S, mut op: F) -> Result<T, backoff::Error<E>>
where
    S: Display,
    E: Display,
    F: FnMut() -> Result<T, backoff::Error<E>>,
{
    op.retry_notify(&mut ExponentialBackoff::default(), |err, _| {
        warn!("{} failed due to {}. Retrying", msg, err);
    })
}
