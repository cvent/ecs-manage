use backoff;
use failure::Error;
use rusoto_core::reactor::RequestDispatcher;
use rusoto_core::ProvideAwsCredentials;
use rusoto_ecr::{DescribeImagesRequest, Ecr, EcrClient, ImageDetail, ImageIdentifier};
use rusoto_ecs::{
    CreateServiceRequest, DescribeServicesError, DescribeServicesRequest,
    DescribeTaskDefinitionError, DescribeTaskDefinitionRequest, Ecs, EcsClient, ListServicesError,
    ListServicesRequest, Service, UpdateServiceError, UpdateServiceRequest,
};
use rusoto_elbv2::{
    DescribeTargetGroupsError, DescribeTargetGroupsInput, Elb, ElbClient, TargetGroup,
};

use args::ServiceModification;
use helpers;

pub fn service_name(service: &Service) -> Result<String, Error> {
    match service.service_name {
        Some(ref service_name) => Ok(service_name.to_owned()),
        None => Err(format_err!("No service name found for {:?}", service)),
    }
}

pub fn compare_services<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    source_cluster: String,
    destination_cluster: String,
) -> Result<Vec<Service>, Error> {
    let source_services = describe_services(&ecs_client, source_cluster)?;
    let destination_services = describe_services(&ecs_client, destination_cluster)?;

    let destination_names = destination_services
        .into_iter()
        .map(|s| s.service_name)
        .collect::<Vec<Option<String>>>();

    Ok(source_services
        .into_iter()
        .filter(|s| !destination_names.contains(&s.service_name))
        .collect::<Vec<Service>>())
}

pub fn list_services<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    cluster: String,
) -> Result<Vec<String>, Error> {
    let mut token = Some(String::new());

    let mut services = Vec::new();

    while token.is_some() {
        let res = helpers::retry_log(format!("listing services in {}", cluster), || {
            ecs_client
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

pub fn describe_service<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    cluster: String,
    service: String,
) -> Result<Service, Error> {
    let res = helpers::retry_log(format!("Describing {}/{}", cluster, service), || {
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

pub fn describe_services<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    cluster: String,
) -> Result<Vec<Service>, Error> {
    list_services(&ecs_client, cluster.clone())?
        .into_iter()
        .map(|service| describe_service(&ecs_client, cluster.clone(), service))
        .collect()
}

pub fn create_service<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    cluster: String,
    from_service: Service,
    role_suffix: Option<String>,
) -> Result<Option<Service>, Error> {
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

    let service_name = service_name(&from_service)?;

    println!(
        "Creating {}/{} with role: {:?}",
        cluster, service_name, role
    );

    let desired_count = from_service.clone().desired_count.ok_or(format_err!(
        "No desired count found for {:?}",
        &service_name
    ))?;

    let task_definition = from_service.clone().task_definition.ok_or(format_err!(
        "No task definition found for {}",
        &service_name
    ))?;

    let response = ecs_client
        .create_service(&CreateServiceRequest {
            client_token: None,
            cluster: Some(cluster.clone()),
            deployment_configuration: from_service.deployment_configuration.clone(),
            desired_count,
            health_check_grace_period_seconds: from_service.health_check_grace_period_seconds,
            launch_type: from_service.launch_type.clone(),
            load_balancers: from_service.load_balancers.clone(),
            network_configuration: from_service.network_configuration.clone(),
            placement_constraints: from_service.placement_constraints.clone(),
            placement_strategy: from_service.placement_strategy.clone(),
            platform_version: from_service.platform_version.clone(),
            role: role.clone(),
            service_name: service_name.clone(),
            task_definition: task_definition.clone(),
        })
        .sync();

    match response {
        Ok(response) => {
            let service = response
                .service
                .ok_or(format_err!("Tried to create service, but nothing returned"))?;
            Ok(Some(service))
        }
        Err(e) => {
            error!("Failed to create {}, due to {:?}", service_name, e);
            Ok(None)
        }
    }
}

pub fn update_service<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    cluster: String,
    service: Service,
    modification: ServiceModification,
) -> Result<Service, Error> {
    let service_name = service_name(&service)?;

    let template_req = UpdateServiceRequest {
        cluster: Some(cluster.clone()),
        deployment_configuration: None,
        desired_count: None,
        force_new_deployment: None,
        health_check_grace_period_seconds: None,
        network_configuration: None,
        platform_version: None,
        service: service_name.clone(),
        task_definition: None,
    };

    let req = match modification {
        ServiceModification::DesiredCount { count } => {
            println!(
                "Updating {}/{}'s desired count to {}. It was {:?}",
                cluster, service_name, count, service.desired_count
            );

            UpdateServiceRequest {
                desired_count: Some(count),
                ..template_req
            }
        }
    };

    helpers::retry_log(
        format!(
            "Updating {}/{} to {:?}",
            cluster, service_name, modification
        ),
        || {
            ecs_client.update_service(&req).sync().map_err(|e| match e {
                UpdateServiceError::Unknown(s) => {
                    if s == r#"{"__type":"ThrottlingException","message":"Rate exceeded"}"# {
                        backoff::Error::Transient(UpdateServiceError::Unknown(s))
                    } else {
                        backoff::Error::Permanent(UpdateServiceError::Unknown(s))
                    }
                }
                _ => backoff::Error::Permanent(e),
            })
        },
    )?.service
        .ok_or(format_err!("Tried to update service, but nothing returned"))
}

pub fn audit_service<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    ecr_client: &EcrClient<P, RequestDispatcher>,
    elb_client: &ElbClient<P, RequestDispatcher>,
    service: &Service,
) -> Result<Vec<String>, Error> {
    let audit = hashmap![
        "Invalid ECR images" => service_ecr_images(&ecs_client, &ecr_client, &service)?.iter().any(|r| r.is_err()),
        "Invalid Target groups" => service_target_groups(&elb_client, &service)?.iter().any(|r| r.is_err()),
        "Less than desired" => service.running_count.unwrap_or(0) < service.desired_count.unwrap_or(0)
    ];

    Ok(audit
        .into_iter()
        .filter(|(_, v)| *v)
        .map(|(k, _)| String::from(k))
        .collect::<Vec<String>>())
}

pub fn service_ecr_images<P: ProvideAwsCredentials + 'static>(
    ecs_client: &EcsClient<P, RequestDispatcher>,
    ecr_client: &EcrClient<P, RequestDispatcher>,
    service: &Service,
) -> Result<Vec<Result<ImageDetail, Error>>, Error> {
    match service.task_definition {
        Some(ref task_definition) => {
            let task_definition =
                helpers::retry_log(format!("describing {}", task_definition), || {
                    ecs_client
                        .describe_task_definition(&DescribeTaskDefinitionRequest {
                            task_definition: task_definition.clone(),
                        })
                        .sync()
                        .map_err(|e| match e {
                            DescribeTaskDefinitionError::Unknown(s) => if s
                                == r#"{"__type":"ThrottlingException","message":"Rate exceeded"}"# {
                                backoff::Error::Transient(DescribeTaskDefinitionError::Unknown(s))
                            } else {
                                backoff::Error::Permanent(DescribeTaskDefinitionError::Unknown(s))
                            },
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

pub fn service_target_groups<P: ProvideAwsCredentials + 'static>(
    elb_client: &ElbClient<P, RequestDispatcher>,
    service: &Service,
) -> Result<Vec<Result<TargetGroup, Error>>, Error> {
    match service.load_balancers {
        Some(ref load_balancers) => {
            let mut target_groups = Vec::new();

            for target_group_arn in load_balancers.iter().map(|lb| lb.target_group_arn.clone()) {
                match target_group_arn {
                    Some(target_group_arn) => {
                        let target_groups_res =
                            helpers::retry_log(format!("describing {}", target_group_arn), || {
                                elb_client
                                    .describe_target_groups(&DescribeTargetGroupsInput {
                                        load_balancer_arn: None,
                                        marker: None,
                                        names: None,
                                        page_size: None,
                                        target_group_arns: Some(vec![target_group_arn.clone()]),
                                    })
                                    .sync()
                                    .map_err(|e| match e {
                                        DescribeTargetGroupsError::Unknown(s) => {
                                            if s.contains("<Code>Throttling</Code>") {
                                                backoff::Error::Transient(
                                                    DescribeTargetGroupsError::Unknown(s),
                                                )
                                            } else {
                                                backoff::Error::Permanent(
                                                    DescribeTargetGroupsError::Unknown(s),
                                                )
                                            }
                                        }
                                        _ => backoff::Error::Permanent(e),
                                    })
                            });

                        match target_groups_res {
                            Ok(target_groups_res) => match target_groups_res.target_groups {
                                Some(mut target_group_details) => {
                                    match target_group_details.pop() {
                                        Some(target_group_detail) => {
                                            target_groups.push(Ok(target_group_detail));
                                        }
                                        None => {}
                                    }
                                }
                                None => {}
                            },
                            Err(e) => target_groups.push(Err(e.into())),
                        }
                    }
                    None => {}
                }
            }

            Ok(target_groups)
        }
        None => Ok(Vec::new()),
    }
}
