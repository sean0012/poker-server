use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::models::Game;

pub type Games = Arc<Mutex<HashMap<String, Game>>>;

pub static NEXT_GAME_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
pub static NEXT_ACTIVITY_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
