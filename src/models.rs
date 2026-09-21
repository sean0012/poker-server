use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Gaze {
    Opponent,
    //Table
}

#[derive(Clone, Serialize)]
pub struct Player {
    pub seat: usize,
    pub connected: bool,
    pub initial_chips: u32,
    pub current_chips: u32,
    pub gaze: Gaze,
}

#[derive(Clone, Serialize)]
pub struct Room {
    pub name: String,
    pub deck_size: u32,
    pub remaining_cards: u32,
    pub players: [Player; 2],
    pub spectators: usize,
    pub pot: u32,
    pub history: Vec<String>,
}

#[derive(Deserialize)]
pub struct CreateGame {
    pub name: String,
    pub deck_size: u32,
    pub initial_chips: [u32; 2],
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Player,
    Spectator,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Joined {
        role: Role,
        seat: Option<usize>,
        room: Room,
    },
    Error {
        message: &'static str,
    },
}
