use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::models::Room;

pub type Rooms = Arc<Mutex<HashMap<String, Room>>>;

pub static NEXT_ROOM_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
pub static NEXT_ACTIVITY_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
