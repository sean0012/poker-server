use crate::{
    models::{CreateGame, Gaze, Player, Room},
    state::Rooms,
};

#[derive(Clone)]
pub struct RoomService {
    rooms: Rooms,
}

impl RoomService {
    pub fn new(rooms: Rooms) -> Self {
        Self { rooms }
    }

    pub async fn room_exists(&self, name: &str) -> bool {
        self.rooms.lock().await.contains_key(name)
    }

    pub async fn create_room(&self, input: CreateGame) -> Result<Room, &'static str> {
        let name = input.name.trim().to_string();
        let mut rooms = self.rooms.lock().await;

        if rooms.contains_key(name.as_str()) {
            return Err("Room already exists");
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
        Ok(room)
    }

    pub async fn get_room(&self, name: &str) -> Result<Room, &'static str> {
        let rooms = self.rooms.lock().await;

        rooms.get(name).cloned().ok_or("Room not found")
    }

    pub async fn list_rooms(&self) -> Vec<Room> {
        let rooms = self.rooms.lock().await;

        rooms.values().cloned().collect()
    }

    pub async fn join_player(&self, name: &str) -> Result<(usize, Room), &'static str> {
        let mut rooms = self.rooms.lock().await;
        let room = rooms.get_mut(name).ok_or("Room not found")?;

        let player = room
            .players
            .iter_mut()
            .find(|player| !player.connected)
            .ok_or("Room is full")?;

        player.connected = true;
        Ok((player.seat, room.clone()))
    }

    pub async fn join_spectator(&self, name: &str) -> Result<Room, &'static str> {
        let mut rooms = self.rooms.lock().await;
        let room = rooms.get_mut(name).ok_or("Room not found")?;
        room.spectators += 1;
        Ok(room.clone())
    }

    pub async fn leave_connection(&self, name: &str, seat: Option<usize>) {
        let mut rooms = self.rooms.lock().await;

        if let Some(room) = rooms.get_mut(name) {
            match seat {
                Some(seat) => room.players[seat - 1].connected = false,
                None => room.spectators = room.spectators.saturating_sub(1),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RoomService;
    use crate::{models::CreateGame, state::Rooms};
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn creates_and_assigns_players_and_spectators() {
        let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));
        let service = RoomService::new(rooms.clone());

        let room = service
            .create_room(CreateGame {
                name: "test_room".to_string(),
                deck_size: 20,
                initial_chips: [100, 200],
            })
            .await
            .unwrap();

        assert_eq!(room.players[0].seat, 1);
        assert_eq!(room.players[1].seat, 2);
        assert_eq!(room.spectators, 0);

        let (player_seat, joined_player_room) = service.join_player("test_room").await.unwrap();
        assert_eq!(player_seat, 1);
        assert!(joined_player_room.players[0].connected);

        let spectator_room = service.join_spectator("test_room").await.unwrap();
        assert_eq!(spectator_room.spectators, 1);

        service.leave_connection("test_room", Some(1)).await;
        let room_after_leave = service.get_room("test_room").await.unwrap();
        assert!(!room_after_leave.players[0].connected);
    }

    #[tokio::test]
    async fn lists_all_created_rooms() {
        let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));
        let service = RoomService::new(rooms.clone());

        service
            .create_room(CreateGame {
                name: "room_a".to_string(),
                deck_size: 20,
                initial_chips: [100, 200],
            })
            .await
            .unwrap();
        service
            .create_room(CreateGame {
                name: "room_b".to_string(),
                deck_size: 30,
                initial_chips: [50, 150],
            })
            .await
            .unwrap();

        let rooms = service.list_rooms().await;
        assert_eq!(rooms.len(), 2);
        assert!(rooms.iter().any(|room| room.name == "room_a"));
        assert!(rooms.iter().any(|room| room.name == "room_b"));
    }
}
