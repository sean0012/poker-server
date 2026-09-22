use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    middleware,
    response::Response,
    routing::get,
};
use tokio::sync::Mutex;

use crate::{
    log::{log_activity, log_request},
    models::{CreateGame, Role, Room, ServerEvent},
    service::RoomService,
    state::{NEXT_ACTIVITY_ID, Rooms},
};

const MAX_ROOM_NAME_LENGTH: usize = 40;

pub async fn run() {
    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));

    let app = Router::new()
        .route("/rooms", get(list_rooms).post(create_game))
        .route("/rooms/{room}", get(get_room_info))
        .route("/rooms/{room}/player", get(join_as_player))
        .route("/rooms/{room}/spectator", get(join_as_spectator))
        .layer(middleware::from_fn(log_request))
        .with_state(rooms);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("포트 바인딩 실패");

    println!("Server listening on port 3000");

    axum::serve(listener, app).await.expect("서버 실행 실패");
}

async fn create_game(
    State(rooms): State<Rooms>,
    Json(input): Json<CreateGame>,
) -> Result<(StatusCode, Json<Room>), ApiError> {
    let name = input.name.trim().to_string();

    if name.is_empty() || name.chars().count() > MAX_ROOM_NAME_LENGTH {
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

    let service = RoomService::new(rooms);
    let room = service
        .create_room(input)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"))?;

    log_activity(
        "room_created",
        serde_json::json!({
            "room": room.id, "name": room.name, "deck_size": room.deck_size, "initial_chips": room.players.iter().map(|player| player.initial_chips).collect::<Vec<_>>(),
        }),
    );

    Ok((StatusCode::CREATED, Json(room)))
}

async fn list_rooms(State(rooms): State<Rooms>) -> Json<Vec<Room>> {
    let service = RoomService::new(rooms);
    Json(service.list_rooms().await)
}

async fn get_room_info(
    Path(id): Path<String>,
    State(rooms): State<Rooms>,
) -> Result<Json<Room>, ApiError> {
    let service = RoomService::new(rooms);
    let room = service
        .get_room(&id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Room not found"))?;

    Ok(Json(room))
}

async fn join_as_player(
    Path(id): Path<String>,
    State(rooms): State<Rooms>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    join(id, rooms, ws, Role::Player).await
}

async fn join_as_spectator(
    Path(id): Path<String>,
    State(rooms): State<Rooms>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    join(id, rooms, ws, Role::Spectator).await
}

async fn join(
    name: String,
    rooms: Rooms,
    ws: WebSocketUpgrade,
    role: Role,
) -> Result<Response, ApiError> {
    let service = RoomService::new(rooms.clone());
    if !service.room_exists(&name).await {
        return Err((StatusCode::NOT_FOUND, "Room not found"));
    }

    Ok(ws.on_upgrade(move |socket| handle_connection(socket, name, rooms, role)))
}

async fn handle_connection(mut socket: WebSocket, name: String, rooms: Rooms, role: Role) {
    let connection_id = NEXT_ACTIVITY_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let activity = |event: &str, seat: Option<usize>, details: serde_json::Value| {
        log_activity(
            event,
            serde_json::json!({
                "connection_id": connection_id, "room": name, "role": role,
                "seat": seat, "details": details,
            }),
        );
    };
    activity("websocket_connected", None, serde_json::json!({}));
    let admission = {
        let service = RoomService::new(rooms.clone());
        match role {
            Role::Player => match service.join_player(&name).await {
                Ok((seat, room)) => Ok((Some(seat), room)),
                Err(message) => Err(message),
            },
            Role::Spectator => match service.join_spectator(&name).await {
                Ok(room) => Ok((None, room)),
                Err(message) => Err(message),
            },
        }
    };

    let (seat, snapshot) = match admission {
        Ok(value) => value,
        Err(message) => {
            activity(
                "join_rejected",
                None,
                serde_json::json!({"reason": message}),
            );
            let event = ServerEvent::Error { message };
            let _ = send_event(&mut socket, &event).await;
            let _ = socket.send(Message::Close(None)).await;
            activity(
                "websocket_disconnected",
                None,
                serde_json::json!({"reason": "join_rejected"}),
            );
            return;
        }
    };

    activity("room_joined", seat, serde_json::json!({}));
    let event = ServerEvent::Joined {
        role,
        seat,
        room: snapshot,
    };

    let mut disconnect_reason = "stream_ended";
    match send_event(&mut socket, &event).await {
        Ok(()) => {
            activity(
                "websocket_sent",
                seat,
                serde_json::json!({"type": "joined"}),
            );
            while let Some(incoming) = socket.recv().await {
                match incoming {
                    Ok(message) => {
                        let (kind, bytes) = match &message {
                            Message::Text(value) => ("text", value.len()),
                            Message::Binary(value) => ("binary", value.len()),
                            Message::Ping(value) => ("ping", value.len()),
                            Message::Pong(value) => ("pong", value.len()),
                            Message::Close(_) => ("close", 0),
                        };
                        activity(
                            "websocket_received",
                            seat,
                            serde_json::json!({"type": kind, "bytes": bytes}),
                        );
                        if matches!(message, Message::Close(_)) {
                            disconnect_reason = "client_close";
                            break;
                        }
                    }
                    Err(error) => {
                        activity(
                            "websocket_error",
                            seat,
                            serde_json::json!({"operation": "receive", "error": error.to_string()}),
                        );
                        disconnect_reason = "receive_error";
                        break;
                    }
                }
            }
        }
        Err(error) => {
            activity(
                "websocket_error",
                seat,
                serde_json::json!({"operation": "send", "error": error.to_string()}),
            );
            disconnect_reason = "send_error";
        }
    }

    RoomService::new(rooms).leave_connection(&name, seat).await;
    activity("room_left", seat, serde_json::json!({}));
    activity(
        "websocket_disconnected",
        seat,
        serde_json::json!({"reason": disconnect_reason}),
    );
}

async fn send_event(socket: &mut WebSocket, event: &ServerEvent) -> Result<(), axum::Error> {
    let text = serde_json::to_string(event).expect("서버 이벤트 JSON 직렬화 실패");

    socket.send(Message::Text(text.into())).await
}

type ApiError = (StatusCode, &'static str);
