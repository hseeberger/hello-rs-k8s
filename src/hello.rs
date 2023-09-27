use anyhow::{anyhow, Context as AnyhowContext, Result};
use k8s_openapi::{
    api::{
        apps::v1::{Deployment, DeploymentSpec},
        core::v1::{Container, ContainerPort, PodSpec, PodTemplateSpec},
    },
    apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition,
    apimachinery::pkg::apis::meta::v1::LabelSelector,
};
use kube::{
    api::{DeleteParams, Patch, PatchParams, PostParams},
    core::{CustomResourceExt, ObjectMeta},
    runtime::{controller::Action, finalizer},
    Api, Client, CustomResource, ResourceExt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use thiserror::Error;
use tracing::{debug, error, info};

const HELLO_FINALIZER: &str = "hellos.hello.heikoseeberger.de";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, CustomResource)]
#[kube(
    group = "hello.heikoseeberger.de",
    version = "v0",
    kind = "Hello",
    derive = "PartialEq",
    namespaced
)]
pub struct HelloSpec {
    replicas: i32, // this is the type used in the deployment spec!
}

impl Hello {
    async fn reconcile(&self, cx: Arc<Context>) -> Result<Action, Error> {
        debug!(?self, "reconciling");

        let name = self.name_any();
        let ns = self.namespace().context("namespace")?;
        let labels = BTreeMap::from_iter([("app".to_string(), name.clone())]);

        let deployment = Deployment {
            metadata: ObjectMeta {
                name: Some(name.clone()),
                namespace: Some(ns.clone()),
                labels: Some(labels.clone()),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                selector: LabelSelector {
                    match_expressions: None,
                    match_labels: Some(labels.clone()),
                },
                replicas: Some(self.spec.replicas),
                template: PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        labels: Some(labels),
                        ..Default::default()
                    }),
                    spec: Some(PodSpec {
                        containers: vec![Container {
                            name,
                            image: Some("hseeberger/hello-rs:0.1.10".to_string()),
                            ports: Some(vec![ContainerPort {
                                container_port: 80,
                                name: Some("http".to_string()),
                                ..Default::default()
                            }]),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let deployment_api = Api::<Deployment>::namespaced(cx.client.clone(), &ns);
        let result = deployment_api
            .create(&PostParams::default(), &deployment)
            .await;
        debug!(?result, "tried to create deployment");

        let action = match result {
            Ok(_) => Ok(Action::requeue(cx.config.requeue_reconcile_after)),
            Err(kube::Error::Api(res)) if res.code == 409 => Ok(Action::await_change()),
            Err(error) => Err(error),
        }
        .context("create deployment")?;
        Ok(action)
    }

    async fn cleanup(&self, cx: Arc<Context>) -> Result<Action, Error> {
        debug!(?self, "cleaning up");

        let name = self.name_any();
        let ns = self.namespace().context("namespace")?;

        let deployment_api = Api::<Deployment>::namespaced(cx.client.clone(), &ns);
        let result = deployment_api.delete(&name, &DeleteParams::default()).await;
        debug!(?result, "tried to delete deployment");

        let action = match result {
            Ok(_) => Ok(Action::await_change()),
            Err(kube::Error::Api(res)) if res.code == 404 => Ok(Action::await_change()),
            Err(error) => Err(error),
        }
        .context("delete deployment")?;
        Ok(action)
    }
}

#[derive(Clone)]
pub struct Context {
    config: Config,
    client: Client,
}

impl Context {
    pub fn new(config: Config, client: Client) -> Self {
        Self { config, client }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    #[serde(with = "humantime_serde")]
    requeue_reconcile_after: Duration,

    #[serde(with = "humantime_serde")]
    requeue_error_after: Duration,
}

#[derive(Debug, Error)]
#[error(transparent)]
pub struct Error(#[from] anyhow::Error);

pub async fn register_crd(client: Client) -> Result<CustomResourceDefinition> {
    info!("registering Hello CRD");

    Api::all(client)
        .patch(
            "hellos.hello.heikoseeberger.de",
            &PatchParams::apply("hello-rs-k8s"),
            &Patch::Apply(Hello::crd()),
        )
        .await
        .context("patch hellos.hello.heikoseeberger.de CRD")
}

#[allow(unused)]
pub async fn delete_crd(client: Client) -> Result<()> {
    info!("deleting Hello CRD");

    Api::<CustomResourceDefinition>::all(client)
        .delete("hellos.hello.heikoseeberger.de", &DeleteParams::default())
        .await
        .map(|_| ())
        .context("delete hellos.hello.heikoseeberger.de CRD")
}

pub async fn reconcile(hello: Arc<Hello>, cx: Arc<Context>) -> Result<Action, Error> {
    let ns = hello
        .namespace()
        .ok_or_else(|| anyhow!("Echo must be namespaced"))?;

    let api = Api::<Hello>::namespaced(cx.client.clone(), &ns);
    let action = finalizer(&api, HELLO_FINALIZER, hello, |evt| async {
        match evt {
            finalizer::Event::Apply(hello) => hello.reconcile(cx).await,
            finalizer::Event::Cleanup(hello) => hello.cleanup(cx).await,
        }
    })
    .await
    .context("finalizer")?;
    Ok(action)
}

pub fn error_policy(hello: Arc<Hello>, error: &Error, cx: Arc<Context>) -> Action {
    error!(error = format!("{error:#}"), ?hello, "handling error");

    Action::requeue(cx.config.requeue_error_after)
}
