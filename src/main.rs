use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

type Rooms = Arc<Mutex<HashMap<String, Room>>>;
type ApiError = (StatusCode, &'static str);

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Gaze {
    Opponent,
    //Table
}

#[derive(Clone, Serialize)]
struct Player {
    seat: usize,
    connected: bool,
    initial_chips: u32,
    current_chips: u32,
    gaze: Gaze,
}

#[derive(Clone, Serialize)]
struct Room {
    name: String,
    deck_size: u32,
    remaining_cards: u32,
    players: [Player; 2],
    spectators: usize,
    pot: u32,
    history: Vec<String>,
}

#[derive(Deserialize)]
struct CreateGame {
    name: String,
    deck_size: u32,
    initial_chips: [u32; 2],
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Role {
    Player,
    Spectator,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerEvent {
    Joined {
        role: Role,
        seat: Option<usize>,
        room: Room,
    },
    Error {
        message: &'static str,
    },
}

#[tokio::main]
async fn main() {
    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));

    let app = Router::new()
        .route("/rooms", post(create_game))
        .route("/rooms/{room}", get(get_room_info))
        .route("/rooms/{room}/player", get(join_as_player))
        .route("/rooms/{room}/spectator", get(join_as_spectator))
        .with_state(rooms);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("포트 바인딩 실패");

    println!("Server listening on port 3000");

    axum::serve(listener, app).await.expect("서버 실행 실패");
}

// 게임 방 생성
async fn create_game(
    State(rooms): State<Rooms>,
    Json(input): Json<CreateGame>,
) -> Result<(StatusCode, Json<Room>), ApiError> {
    let name = input.name.trim().to_string();

    if name.is_empty()
        || name.len() > 40
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err((StatusCode::BAD_REQUEST, "Invalid room name"));
    }

    if input.deck_size == 0 || input.deck_size % 10 != 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Deck size must be a positive multiple of 10",
        ));
    }

    if input.initial_chips.contains(&0) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Initial chips must be greater than zero",
        ));
    }

    let mut rooms = rooms.lock().await;

    if rooms.contains_key(&name) {
        return Err((StatusCode::CONFLICT, "Room already exists"));
    }

    let room = Room {
        name: name.clone(),
        deck_size: input.deck_size,
        remaining_cards: input.deck_size,
        players: std::array::from_fn(|index| Player {
            seat: index + 1,
            connected: false,
            initial_chips: input.initial_chips[index],
            current_chips: input.initial_chips[index],
            gaze: Gaze::Opponent,
        }),
        spectators: 0,
        pot: 0,
        history: Vec::new(),
    };

    rooms.insert(name, room.clone());

    Ok((StatusCode::CREATED, Json(room)))
}

// 방 정보 조회
async fn get_room_info(
    Path(name): Path<String>,
    State(rooms): State<Rooms>,
) -> Result<Json<Room>, ApiError> {
    let rooms = rooms.lock().await;

    let room = rooms
        .get(&name)
        .cloned()
        .ok_or((StatusCode::NOT_FOUND, "Room not found"))?;

    Ok(Json(room))
}

// 플레이어 입장
async fn join_as_player(
    Path(name): Path<String>,
    State(rooms): State<Rooms>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    join(name, rooms, ws, Role::Player).await
}

// 관전자 입장
async fn join_as_spectator(
    Path(name): Path<String>,
    State(rooms): State<Rooms>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    join(name, rooms, ws, Role::Spectator).await
}

async fn join(
    name: String,
    rooms: Rooms,
    ws: WebSocketUpgrade,
    role: Role,
) -> Result<Response, ApiError> {
    if !rooms.lock().await.contains_key(&name) {
        return Err((StatusCode::NOT_FOUND, "Room not found"));
    }

    Ok(ws.on_upgrade(move |socket| handle_connection(socket, name, rooms, role)))
}

// 연결이 성립한 뒤 실제 입장 처리
async fn handle_connection(mut socket: WebSocket, name: String, rooms: Rooms, role: Role) {
    let admission = {
        let mut rooms = rooms.lock().await;

        match rooms.get_mut(&name) {
            Some(room) => match role {
                Role::Player => match room.players.iter_mut().find(|p| !p.connected) {
                    Some(player) => {
                        player.connected = true;
                        let seat = player.seat;
                        Ok((Some(seat), room.clone()))
                    }
                    None => Err("Room is full"),
                },
                Role::Spectator => {
                    room.spectators += 1;
                    Ok((None, room.clone()))
                }
            },
            None => Err("Room not found"),
        }
    };

    let (seat, snapshot) = match admission {
        Ok(value) => value,
        Err(message) => {
            let event = ServerEvent::Error { message };
            let _ = send_event(&mut socket, &event).await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    let event = ServerEvent::Joined {
        role,
        seat,
        room: snapshot,
    };

    if send_event(&mut socket, &event).await.is_ok() {
        // 이번 단계에는 클라이언트가 보내는 게임 명령이 없음
        while let Some(incoming) = socket.recv().await {
            match incoming {
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    }

    // 연결 종료 시 자리 또는 관전자 수 정리
    let mut rooms = rooms.lock().await;

    if let Some(room) = rooms.get_mut(&name) {
        match seat {
            Some(seat) => room.players[seat - 1].connected = false,
            None => room.spectators -= 1,
        }
    }
}

async fn send_event(socket: &mut WebSocket, event: &ServerEvent) -> Result<(), axum::Error> {
    let text = serde_json::to_string(event).expect("서버 이벤트 JSON 직렬화 실패");

    socket.send(Message::Text(text.into())).await
}
