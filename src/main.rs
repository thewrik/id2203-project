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
use omnipaxos_storage::{
    persistent_storage::{PersistentStorage, PersistentStorageConfig},
};

use commitlog::LogOptions;
use sled::{Config};

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    net::TcpListener
};
use tokio::{runtime::Builder, sync::mpsc};

mod kv;
mod server;
mod util;

type OmniPaxosKV = OmniPaxos<KeyValue, KVSnapshot, PersistentStorage<KeyValue, KVSnapshot>>;

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

fn with_omnipaxos(omnipaxos : Arc<Mutex<OmniPaxosKV>>) -> impl Filter<Extract = (Arc<Mutex<OmniPaxosKV>>,), Error = Infallible> + Clone {
    warp::any().map(move || omnipaxos.clone())
}
fn with_pid(id : u64) -> impl Filter<Extract = (u64,), Error = Infallible> + Clone {
    warp::any().map(move || id)
}

fn sanitize_post_data(data : KeyValue) -> KeyValue {
    let KeyValue {key, value} = data; 
    let key = format!("{}$0", key.replace("$", ""));// 0 represents added entries by clients
    KeyValue {key, value} // $ is a special character used for separation
}

fn convert_to_hashmap(committed_ents : Vec<LogEntry<KeyValue, KVSnapshot>>, check_tag : char) -> HashMap<String, String> {
    let mut simple_kv_store = HashMap::new();
    for ent in committed_ents {
        match ent {
            LogEntry::Decided(kv) => {
                if (kv.key.chars().last().unwrap() == check_tag){
                    simple_kv_store.insert(kv.key[..kv.key.len() - 2].to_string(), kv.value);
                }
                
            }
            _ => {}
        }
    }
    simple_kv_store
}

fn increase_visit_count(pid: u64, omnipaxos : &Arc<Mutex<OmniPaxosKV>>) -> () {
    let committed_ents = omnipaxos
        .lock()
        .unwrap()
        .read_decided_suffix(0)
        .expect("Failed to read expected entries");

    
    let nonclient_entries = convert_to_hashmap(committed_ents, '1');
    let key = format!("visits_at_server_{}", pid);
    let existing_visits : u64 = 
        match nonclient_entries.get(&key) {
            Some(visits) => 
                visits.to_string().parse().unwrap(),
            None => 0
        };
    
    let entry = KeyValue {
        key : format!("visits_at_server_{}$1", pid),
        value : (existing_visits + 1).to_string()
    };

    omnipaxos
        .lock()
        .unwrap()
        .append(entry)
        .expect("append failed");
}
async fn append_to_kv(data: KeyValue, omnipaxos : Arc<Mutex<OmniPaxosKV>>, pid: u64) -> Result<impl warp::Reply, Infallible> {
    omnipaxos
        .lock()
        .unwrap()
        .append(sanitize_post_data(data))
        .expect("append failed");

    // increase count
    std::thread::sleep(WAIT_DECIDED_TIMEOUT);

    increase_visit_count(pid, &omnipaxos);
    
    Ok(StatusCode::OK)
}

async fn list_all(omnipaxos : Arc<Mutex<OmniPaxosKV>>, pid : u64) -> Result<impl warp::Reply, Infallible> {

    increase_visit_count(pid, &omnipaxos);

    let committed_ents = omnipaxos
        .lock()
        .unwrap()
        .read_decided_suffix(0)
        .expect("Failed to read expected entries");

    let simple_kv_store = convert_to_hashmap(committed_ents, '0');
    Ok(warp::reply::json(&simple_kv_store))

}

async fn list_single_entry(key : String, omnipaxos : Arc<Mutex<OmniPaxosKV>>, pid : u64) -> Result<impl warp::Reply, Infallible> {
    increase_visit_count(pid, &omnipaxos);

    let committed_ents = omnipaxos
        .lock()
        .unwrap()
        .read_decided_suffix(0)
        .expect("Failed to read expected entries");

    
    let simple_kv_store = convert_to_hashmap(committed_ents, '0');

    let mut status = "found";
    let value  = 
        match simple_kv_store.get(&key) {
            Some(value) => value,
            None =>  {
                status = "not found";
                ""
            }
        };
    let mut result : HashMap<String, String> = HashMap::new();

    result.insert(String::from("status"), status.to_string());
    result.insert(String::from("value"), value.to_string());

    Ok(warp::reply::json(&result))

}

async fn list_visit_stats(omnipaxos : Arc<Mutex<OmniPaxosKV>>, pid : u64) -> Result<impl warp::Reply, Infallible> {

    increase_visit_count(pid, &omnipaxos);

    std::thread::sleep(WAIT_DECIDED_TIMEOUT);

    let committed_ents = omnipaxos
        .lock()
        .unwrap()
        .read_decided_suffix(0)
        .expect("Failed to read expected entries");

    let simple_kv_store = convert_to_hashmap(committed_ents, '1');
    Ok(warp::reply::json(&simple_kv_store))

}
fn json_body() -> impl Filter<Extract = (KeyValue,), Error = warp::Rejection> + Clone {
    warp::body::content_length_limit(1024 * 16).and(warp::body::json())
}

fn get_rule_for_pid(pid: u64, op_server_handles: &Server_Handles) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone  {
    let (omnipaxos_id_instance, _) = op_server_handles.get(&pid).unwrap(); 

    let initial_visit_entry = KeyValue {
        key : format!("visits_at_server_{}$1", pid),
        value : (0).to_string()
    };

    omnipaxos_id_instance
        .lock()
        .unwrap()
        .append(initial_visit_entry)
        .expect("append failed");
    
    let post_path = warp::path("post")
        .and(warp::post())
        .and(json_body())
        .and(with_omnipaxos(omnipaxos_id_instance.clone()))
        .and(with_pid(pid))
        .and_then(append_to_kv);

    let get_entire_path = warp::path("get_all")
        .and(warp::get())
        .and(with_omnipaxos(omnipaxos_id_instance.clone()))
        .and(with_pid(pid))
        .and_then(list_all);

    let get_visit_stats = warp::path("get_visit_stats")
        .and(warp::get())
        .and(with_omnipaxos(omnipaxos_id_instance.clone()))
        .and(with_pid(pid))
        .and_then(list_visit_stats);

    let get_path = warp::path!("get" / String)
        .and(warp::get())
        .and(with_omnipaxos(omnipaxos_id_instance.clone()))
        .and(with_pid(pid))
        .and_then(list_single_entry);

    post_path
        .or(get_entire_path)
        .or(get_visit_stats)
        .or(get_path)
        
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

        // Persistent Storage Configuration
        let storage_path = format!("storage/process_{}", pid);   
        let logopts = LogOptions::new(storage_path.to_string());


        let mut storage_config = PersistentStorageConfig::default();
        storage_config.set_path(storage_path.to_string());
        storage_config.set_commitlog_options(logopts);
        
        let storage = PersistentStorage::open(storage_config);

        let omni_paxos: Arc<Mutex<OmniPaxosKV>> =
            Arc::new(Mutex::new(op_config.build(storage)));
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
        // let entry = KeyValue {
        //     key : format!("visits_at_server_{}$1", pid),
        //     value : (0).to_string()
        // };
    
        // omni_paxos
        //     .lock()
        //     .unwrap()
        //     .append(entry)
        op_server_handles.insert(pid, (omni_paxos, join_handle));
    }

    std::thread::sleep(WAIT_LEADER_TIMEOUT);

    tokio::join!(
        warp::serve(get_rule_for_pid(1, &op_server_handles)).run(([127, 0, 0, 1], 3030 + 0)),
        warp::serve(get_rule_for_pid(2, &op_server_handles)).run(([127, 0, 0, 1], 3030 + 1)),
        warp::serve(get_rule_for_pid(3, &op_server_handles)).run(([127, 0, 0, 1], 3030 + 2)),
    );
}
