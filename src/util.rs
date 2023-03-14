use std::time::Duration;

use crate::{
    kv::{KVSnapshot, KeyValue},
};
use tokio::task::JoinHandle;
use std::convert::Infallible;
use omnipaxos_core::{
    messages::Message,
    omni_paxos::*,
    util::{LogEntry, NodeId},
};

use omnipaxos_storage::{
    persistent_storage::PersistentStorage,
};
use crate::SERVERS;

use warp::Filter;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

type OmniPaxosKV = OmniPaxos<KeyValue, KVSnapshot, PersistentStorage<KeyValue, KVSnapshot>>;


pub const BUFFER_SIZE: usize = 10000;
pub const ELECTION_TIMEOUT: Duration = Duration::from_millis(100);
pub const OUTGOING_MESSAGE_PERIOD: Duration = Duration::from_millis(1);

pub const WAIT_LEADER_TIMEOUT: Duration = Duration::from_millis(500);
pub const WAIT_DECIDED_TIMEOUT: Duration = Duration::from_millis(50);

pub fn initialise_channels() -> (
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

pub fn with_omnipaxos(omnipaxos : Arc<Mutex<OmniPaxosKV>>) -> impl Filter<Extract = (Arc<Mutex<OmniPaxosKV>>,), Error = Infallible> + Clone {
    warp::any().map(move || omnipaxos.clone())
}
pub fn with_pid(id : u64) -> impl Filter<Extract = (u64,), Error = Infallible> + Clone {
    warp::any().map(move || id)
}

pub fn sanitize_post_data(data : KeyValue) -> KeyValue {
    let KeyValue {key, value} = data; 
    let key = format!("{}$0", key.replace("$", ""));// 0 represents added entries by clients
    KeyValue {key, value} // $ is a special character used for separation
}

pub fn convert_to_hashmap(committed_ents : Vec<LogEntry<KeyValue, KVSnapshot>>, check_tag : char) -> HashMap<String, String> {
    let mut simple_kv_store = HashMap::new();
    for ent in committed_ents {
        match ent {
            LogEntry::Decided(kv) => {
                if kv.key.chars().last().unwrap() == check_tag {
                    simple_kv_store.insert(kv.key[..kv.key.len() - 2].to_string(), kv.value);
                }
                
            }
            _ => {}
        }
    }
    simple_kv_store
}

pub fn json_body() -> impl Filter<Extract = (KeyValue,), Error = warp::Rejection> + Clone {
    warp::body::content_length_limit(1024 * 16).and(warp::body::json())
}
