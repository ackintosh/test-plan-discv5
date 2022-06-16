mod eclipse;
mod find_node;

use discv5::enr::{CombinedKey, Enr};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use testground::client::Client;
use testground::network_conf::{
    FilterAction, LinkShape, NetworkConfiguration, RoutingPolicyType, DEAFULT_DATA_NETWORK,
};
use testground::RunParameters;
use tokio_stream::StreamExt;

const STATE_NETWORK_CONFIGURED: &str = "state_network_configured";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (client, run_parameters) = testground::client::Client::new().await?;

    // Enable tracing.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new("info"))
        .expect("EnvFilter");
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();

    // ////////////////////////
    // Configure network
    // ////////////////////////
    client.wait_network_initialized().await?;

    client
        .configure_network(NetworkConfiguration {
            network: DEAFULT_DATA_NETWORK.to_owned(),
            ipv4: None,
            ipv6: None,
            enable: true,
            default: LinkShape {
                latency: run_parameters
                    .test_instance_params
                    .get("latency")
                    .ok_or("latency is not specified")?
                    .parse::<u64>()?
                    * 1_000_000, // Translate from millisecond to nanosecond
                jitter: 0,
                bandwidth: 1048576, // 1Mib
                filter: FilterAction::Accept,
                loss: 0.0,
                corrupt: 0.0,
                corrupt_corr: 0.0,
                reorder: 0.0,
                reorder_corr: 0.0,
                duplicate: 0.0,
                duplicate_corr: 0.0,
            },
            rules: None,
            callback_state: STATE_NETWORK_CONFIGURED.to_owned(),
            callback_target: None,
            routing_policy: RoutingPolicyType::DenyAll,
        })
        .await?;

    client
        .barrier(STATE_NETWORK_CONFIGURED, run_parameters.test_instance_count)
        .await?;

    // //////////////////////////////////////////////////////////////
    // Run test case
    // //////////////////////////////////////////////////////////////
    match run_parameters.test_case.clone().as_str() {
        "eclipse-attack-table-poisoning" => {
            eclipse::TablePoisoning::new(run_parameters.clone())
                .run(client.clone())
                .await?
        }
        "find-node" => find_node::find_node(client.clone(), run_parameters.clone()).await?,
        _ => unreachable!(),
    };

    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InstanceInfo {
    // The sequence number of this test instance within the test.
    seq: u64,
    enr: Enr<CombinedKey>,
    is_bootstrap_node: bool,
}

impl InstanceInfo {
    async fn new(
        client: &Client,
        enr: Enr<CombinedKey>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let seq = get_instance_seq(client).await?;

        // NOTE: For now, #1 is bootstrap node.
        let is_bootstrap_node = seq == 1;

        Ok(InstanceInfo {
            seq,
            enr,
            is_bootstrap_node,
        })
    }
}

// Returns the sequence number of this test instance within the test.
async fn get_instance_seq(client: &Client) -> Result<u64, testground::errors::Error> {
    client.signal("get_instance_seq").await
}

async fn get_group_seq(
    client: &Client,
    group_id: &String,
) -> Result<u64, testground::errors::Error> {
    client.signal(format!("get_group_seq_{}", group_id)).await
}

async fn collect_instance_info(
    client: &Client,
    run_parameters: &RunParameters,
    instance_info: &InstanceInfo,
) -> Result<Vec<InstanceInfo>, Box<dyn std::error::Error>> {
    let mut info = publish_and_collect(client, run_parameters, instance_info.clone()).await?;

    if let Some(pos) = info.iter().position(|i| i.seq == instance_info.seq) {
        info.remove(pos);
    }

    Ok(info)
}

async fn publish_and_collect<T: Serialize + DeserializeOwned>(
    client: &Client,
    run_parameters: &RunParameters,
    info: T,
) -> Result<Vec<T>, Box<dyn std::error::Error>> {
    const TOPIC: &str = "publish_and_collect";

    client.publish(TOPIC, serde_json::to_string(&info)?).await?;

    let mut stream = client.subscribe(TOPIC).await;

    let mut vec: Vec<T> = vec![];

    for _ in 0..run_parameters.test_instance_count {
        match stream.next().await {
            Some(Ok(other)) => {
                let info: T = serde_json::from_str(&other)?;
                vec.push(info);
            }
            Some(Err(e)) => return Err(Box::new(e)),
            None => unreachable!(),
        }
    }

    Ok(vec)
}
