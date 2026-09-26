use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Gaze {
    Opponent,
    //Table
}

#[derive(Clone, Serialize)]
pub struct Player {
    pub seat: usize,
    #[serde(skip_serializing)]
    pub user_id: Option<Uuid>,
    pub avatar: String,
    pub connected: bool,
    pub initial_chips: u32,
    pub current_chips: u32,
    pub gaze: Gaze,
}

#[derive(Clone, Serialize)]
pub struct Game {
    pub id: String,
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

#[derive(Serialize)]
pub struct SessionResponse {
    pub user_id: Uuid,
    pub display_name: String,
    pub avatar: String,
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
        game: Game,
    },
    GameUpdated {
        game: Game,
    },
    Error {
        message: &'static str,
    },
}
