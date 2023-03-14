use crate::{
    kv::{KVSnapshot, KeyValue},
    server::OmniPaxosServer,
    util::*,
    handlers::*,
};
use tokio::task::JoinHandle;

use omnipaxos_core::{
    omni_paxos::*,
};

use warp::Filter;

use omnipaxos_storage::{
    persistent_storage::{PersistentStorage, PersistentStorageConfig},
};

use commitlog::LogOptions;


use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::runtime::Builder;

mod kv;
mod server;
mod util;
mod handlers;

type OmniPaxosKV = OmniPaxos<KeyValue, KVSnapshot, PersistentStorage<KeyValue, KVSnapshot>>;

type ServerHandles = HashMap<u64, (Arc<Mutex<OmniPaxosKV>>, JoinHandle<()>)>;

const SERVERS: [u64; 3] = [1, 2, 3];


fn get_rule_for_pid(pid: u64, op_server_handles: &ServerHandles) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone  {
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
    let mut op_server_handles : ServerHandles = HashMap::new();
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

        op_server_handles.insert(pid, (omni_paxos, join_handle));
    }

    std::thread::sleep(WAIT_LEADER_TIMEOUT);

    tokio::join!(
        warp::serve(get_rule_for_pid(1, &op_server_handles)).run(([127, 0, 0, 1], 3030 + 0)),
        warp::serve(get_rule_for_pid(2, &op_server_handles)).run(([127, 0, 0, 1], 3030 + 1)),
        warp::serve(get_rule_for_pid(3, &op_server_handles)).run(([127, 0, 0, 1], 3030 + 2)),
    );
}
