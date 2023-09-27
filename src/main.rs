mod hello;

use crate::hello::Hello;
use anyhow::{Context, Result};
use configured::Configured;
use futures::StreamExt;
use kube::{
    runtime::{watcher, Controller},
    Api, Client,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Debug, Clone, Deserialize)]
struct Config {
    hello: hello::Config,
}

#[tokio::main]
async fn main() {
    if let Err(error) = init_tracing() {
        eprintln!("hello-rs-k8s exited with ERROR: {error:#}");
    }

    if let Err(ref error) = run().await {
        error!(
            error = format!("{error:#}"),
            backtrace = %error.backtrace(),
            "hello-rs-k8s exited with ERROR"
        );
    };
}

async fn run() -> Result<()> {
    let config = Config::load().context("load configuration")?;

    info!(?config, "starting");

    let client = Client::try_default().await.context("create kube client")?;

    hello::register_crd(client.clone()).await?;

    let hello_ctx = hello::Context::new(config.hello, client.clone());

    Controller::<Hello>::new(Api::all(client), watcher::Config::default())
        .run(hello::reconcile, hello::error_policy, Arc::new(hello_ctx))
        .for_each(|result| async move {
            match result {
                Ok(obj) => debug!(?obj, "successfully reconciled"),
                Err(error) => error!(error = format!("{error:#}"), "reconcile failed"),
            }
        })
        .await;

    Ok(())
}

fn init_tracing() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(fmt::layer().json())
        .try_init()
        .context("initialize tracing subscriber")
}
