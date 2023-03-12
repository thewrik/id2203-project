use crate::{
    kv::{KVSnapshot, KeyValue},
    server::OmniPaxosServer,
    util::*,
};
use tokio::task::JoinHandle;
use std::convert::Infallible;
use omnipaxos_core::{
    messages::Message,
    omni_paxos::*,
    util::{LogEntry, NodeId},
};

use warp::{http, Filter, http::StatusCode};
use omnipaxos_storage::memory_storage::MemoryStorage;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    net::TcpListener
};
use tokio::{runtime::Builder, sync::mpsc};

mod kv;
mod server;
mod util;

type OmniPaxosKV = OmniPaxos<KeyValue, KVSnapshot, MemoryStorage<KeyValue, KVSnapshot>>;

type Server_Handles = HashMap<u64, (Arc<Mutex<OmniPaxosKV>>, JoinHandle<()>)>;

const SERVERS: [u64; 3] = [1, 2, 3];

fn initialise_channels() -> (
    HashMap<NodeId, mpsc::Sender<Message<KeyValue, KVSnapshot>>>,
    HashMap<NodeId, mpsc::Receiver<Message<KeyValue, KVSnapshot>>>,
) {
    let mut sender_channels = HashMap::new();
    let mut receiver_channels = HashMap::new();

    for pid in SERVERS {
        let (sender, receiver) = mpsc::channel(BUFFER_SIZE);
        sender_channels.insert(pid, sender);
        receiver_channels.insert(pid, receiver);
    }
    (sender_channels, receiver_channels)
}

fn with_pid(pid: u64) -> impl Filter<Extract = (u64,), Error = Infallible> + Clone {
    warp::any().map(move || pid)
}

fn with_omnipaxos(omnipaxos : Arc<Mutex<OmniPaxosKV>>) -> impl Filter<Extract = (Arc<Mutex<OmniPaxosKV>>,), Error = Infallible> + Clone {
    warp::any().map(move || omnipaxos.clone())
}

async fn append_to_kv(data: KeyValue, id: u64, omnipaxos : Arc<Mutex<OmniPaxosKV>>) -> Result<impl warp::Reply, Infallible> {
    omnipaxos
        .lock()
        .unwrap()
        .append(data)
        .expect("append failed");

    
    Ok(StatusCode::OK)
}

async fn list_all(id: u64, omnipaxos : Arc<Mutex<OmniPaxosKV>>) -> Result<impl warp::Reply, Infallible> {

    let committed_ents = omnipaxos
        .lock()
        .unwrap()
        .read_decided_suffix(0)
        .expect("Failed to read expected entries");

    let mut simple_kv_store = HashMap::new();
    for ent in committed_ents {
        match ent {
            LogEntry::Decided(kv) => {
                simple_kv_store.insert(kv.key, kv.value);
            }
            _ => {}
        }
    }
    Ok(warp::reply::json(&simple_kv_store))

}
fn json_body() -> impl Filter<Extract = (KeyValue,), Error = warp::Rejection> + Clone {
    warp::body::content_length_limit(1024 * 16).and(warp::body::json())
}

fn get_rule_for_pid(pid: u64, op_server_handles: &Server_Handles) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone  {
    let (omnipaxos_id_instance, _) = op_server_handles.get(&pid).unwrap(); 
    
    let post_path = warp::path("post")
        .and(warp::post())
        .and(json_body())
        .and(with_pid(pid))
        .and(with_omnipaxos(omnipaxos_id_instance.clone()))
        .and_then(append_to_kv);

    let get_entire_path = warp::path("get_all")
        .and(warp::get())
        .and(with_pid(pid))
        .and(with_omnipaxos(omnipaxos_id_instance.clone()))
        .and_then(list_all);

    post_path.
        or(get_entire_path)
}

#[tokio::main]
async fn main() {
    let runtime = Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();

    let configuration_id = 1;
    let mut op_server_handles : Server_Handles = HashMap::new();
    let (sender_channels, mut receiver_channels) = initialise_channels();

    for pid in SERVERS {
        let peers = SERVERS.iter().filter(|&&p| p != pid).copied().collect();
        
        let op_config = OmniPaxosConfig {
            pid,
            configuration_id,
            peers,
            ..Default::default()
        };
        let omni_paxos: Arc<Mutex<OmniPaxosKV>> =
            Arc::new(Mutex::new(op_config.build(MemoryStorage::default())));
        let mut op_server = OmniPaxosServer {
            omni_paxos: Arc::clone(&omni_paxos),
            incoming: receiver_channels.remove(&pid).unwrap(),
            outgoing: sender_channels.clone(),
        };
        let join_handle = runtime.spawn({
            async move {
                op_server.run().await;
            }
        });
        op_server_handles.insert(pid, (omni_paxos, join_handle));
    }

    std::thread::sleep(WAIT_LEADER_TIMEOUT);

    tokio::join!(
        warp::serve(get_rule_for_pid(1, &op_server_handles)).run(([127, 0, 0, 1], 3030 + 0)),
        warp::serve(get_rule_for_pid(2, &op_server_handles)).run(([127, 0, 0, 1], 3030 + 1)),
        warp::serve(get_rule_for_pid(3, &op_server_handles)).run(([127, 0, 0, 1], 3030 + 2)),
    );
}
