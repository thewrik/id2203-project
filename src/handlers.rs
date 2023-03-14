use crate::{
    kv::{KVSnapshot, KeyValue},
    util::*,
};
use tokio::task::JoinHandle;
use std::convert::Infallible;
use omnipaxos_core::{
    omni_paxos::*,
};

use omnipaxos_storage::persistent_storage::PersistentStorage;

use warp::http::StatusCode;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};


type OmniPaxosKV = OmniPaxos<KeyValue, KVSnapshot, PersistentStorage<KeyValue, KVSnapshot>>;

pub fn increase_visit_count(pid: u64, omnipaxos : &Arc<Mutex<OmniPaxosKV>>) -> () {
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
        .set_priority(existing_visits + 1);

    omnipaxos
        .lock()
        .unwrap()
        .append(entry)
        .expect("append failed");

    std::thread::sleep(WAIT_DECIDED_TIMEOUT);
    

    let leader = omnipaxos
        .lock()
        .unwrap()
        .get_current_leader()
        .expect("Failed to get leader");

    println!("Current leader: {}", leader);
}
pub async fn append_to_kv(data: KeyValue, omnipaxos : Arc<Mutex<OmniPaxosKV>>, pid: u64) -> Result<impl warp::Reply, Infallible> {
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

pub async fn list_all(omnipaxos : Arc<Mutex<OmniPaxosKV>>, pid : u64) -> Result<impl warp::Reply, Infallible> {

    increase_visit_count(pid, &omnipaxos);

    let committed_ents = omnipaxos
        .lock()
        .unwrap()
        .read_decided_suffix(0)
        .expect("Failed to read expected entries");

    let simple_kv_store = convert_to_hashmap(committed_ents, '0');
    Ok(warp::reply::json(&simple_kv_store))

}

pub async fn list_single_entry(key : String, omnipaxos : Arc<Mutex<OmniPaxosKV>>, pid : u64) -> Result<impl warp::Reply, Infallible> {
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

pub async fn list_visit_stats(omnipaxos : Arc<Mutex<OmniPaxosKV>>, pid : u64) -> Result<impl warp::Reply, Infallible> {

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