use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::Response,
    routing::get,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};

type Rooms = Arc<Mutex<HashMap<String, broadcast::Sender<String>>>>;

#[derive(Deserialize)]
struct CreateRoom {
    name: String,
}

#[derive(Serialize)]
struct RoomInfo {
    name: String,
    members: usize,
}

#[tokio::main]
async fn main() {
    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));

    let app = Router::new()
        .route("/rooms", get(list_rooms).post(create_room))
        .route("/rooms/{room}/join", get(join_room))
        .with_state(rooms);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("포트 바인딩 실패");

    println!("Server listening on port 3000");

    axum::serve(listener, app).await.expect("서버 실행 실패");
}

// 방 목록 조회
async fn list_rooms(State(rooms): State<Rooms>) -> Json<Vec<RoomInfo>> {
    let rooms = rooms.lock().await;

    let mut list: Vec<RoomInfo> = rooms
        .iter()
        .map(|(name, tx)| RoomInfo {
            name: name.clone(),
            members: tx.receiver_count(),
        })
        .collect();

    list.sort_by(|a, b| a.name.cmp(&b.name));

    Json(list)
}

// 방 생성
async fn create_room(
    State(rooms): State<Rooms>,
    Json(input): Json<CreateRoom>,
) -> Result<(StatusCode, Json<RoomInfo>), (StatusCode, &'static str)> {
    let name = input.name.trim().to_string();

    // URL 경로로 사용하므로 영문, 숫자, -, _만 허용
    if name.is_empty()
        || name.len() > 40
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Room name must be 1-40 characters: letters, digits, - or _",
        ));
    }

    let mut rooms = rooms.lock().await;

    if rooms.contains_key(&name) {
        return Err((StatusCode::CONFLICT, "Room already exists"));
    }

    let (tx, _) = broadcast::channel::<String>(100);
    rooms.insert(name.clone(), tx);

    Ok((StatusCode::CREATED, Json(RoomInfo { name, members: 0 })))
}

// 존재하는 방에 WebSocket으로 입장
async fn join_room(
    Path(room): Path<String>,
    State(rooms): State<Rooms>,
    ws: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    let tx = {
        let rooms = rooms.lock().await;

        rooms.get(&room).cloned().ok_or(StatusCode::NOT_FOUND)?
    };

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, tx)))
}

// 접속자 한 명의 채팅 처리
async fn handle_socket(mut socket: WebSocket, tx: broadcast::Sender<String>) {
    let mut rx = tx.subscribe();

    loop {
        tokio::select! {
            // 접속자가 보낸 메시지 → 같은 방에 배포
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let _ = tx.send(text.to_string());
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                        break;
                    }
                    _ => {}
                }
            }

            // 방의 메시지 → 접속자에게 전달
            outgoing = rx.recv() => {
                match outgoing {
                    Ok(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    // 함수가 끝나면 rx가 자동 해제되어 접속자 수가 감소
}
