use crate::{
    models::{CreateGame, Game, Gaze, Player},
    state::{Games, NEXT_GAME_ID},
};

#[derive(Clone)]
pub struct GameService {
    games: Games,
}

impl GameService {
    pub fn new(games: Games) -> Self {
        Self { games }
    }

    pub async fn game_exists(&self, id: &str) -> bool {
        self.games.lock().await.contains_key(id)
    }

    pub async fn create_game(&self, input: CreateGame) -> Result<Game, &'static str> {
        let name = input.name.trim().to_string();
        let mut games = self.games.lock().await;
        let id = NEXT_GAME_ID
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string();

        let game = Game {
            id: id.clone(),
            name: name.clone(),
            deck_size: input.deck_size,
            remaining_cards: input.deck_size,
            players: std::array::from_fn(|index| Player {
                seat: index + 1,
                guest_id: None,
                connected: false,
                initial_chips: input.initial_chips[index],
                current_chips: input.initial_chips[index],
                gaze: Gaze::Opponent,
            }),
            spectators: 0,
            pot: 0,
            history: Vec::new(),
        };

        games.insert(id, game.clone());
        Ok(game)
    }

    pub async fn get_game(&self, id: &str) -> Result<Game, &'static str> {
        let games = self.games.lock().await;

        games.get(id).cloned().ok_or("Game not found")
    }

    pub async fn list_games(&self) -> Vec<Game> {
        let games = self.games.lock().await;

        games.values().cloned().collect()
    }

    pub async fn join_player(
        &self,
        id: &str,
        guest_id: &str,
    ) -> Result<(usize, Game), &'static str> {
        let mut games = self.games.lock().await;
        let game = games.get_mut(id).ok_or("Game not found")?;

        let player = if let Some(player) = game
            .players
            .iter_mut()
            .find(|player| player.guest_id.as_deref() == Some(guest_id))
        {
            if player.connected {
                return Err("Player is already connected");
            }
            player
        } else {
            game.players
                .iter_mut()
                .find(|player| !player.connected)
                .ok_or("Game is full")?
        };

        player.guest_id = Some(guest_id.to_string());
        player.connected = true;
        Ok((player.seat, game.clone()))
    }

    pub async fn join_spectator(&self, id: &str) -> Result<Game, &'static str> {
        let mut games = self.games.lock().await;
        let game = games.get_mut(id).ok_or("Game not found")?;
        game.spectators += 1;
        Ok(game.clone())
    }

    pub async fn leave_connection(&self, id: &str, seat: Option<usize>) -> Option<Game> {
        let mut games = self.games.lock().await;

        if let Some(game) = games.get_mut(id) {
            match seat {
                Some(seat) => game.players[seat - 1].connected = false,
                None => game.spectators = game.spectators.saturating_sub(1),
            }
            Some(game.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GameService;
    use crate::{models::CreateGame, state::Games};
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn creates_and_assigns_players_and_spectators() {
        let games: Games = Arc::new(Mutex::new(HashMap::new()));
        let service = GameService::new(games.clone());

        let game = service
            .create_game(CreateGame {
                name: "test_game".to_string(),
                deck_size: 20,
                initial_chips: [100, 200],
            })
            .await
            .unwrap();

        assert_eq!(game.players[0].seat, 1);
        assert_eq!(game.players[1].seat, 2);
        assert_eq!(game.spectators, 0);
        assert_eq!(game.name, "test_game");

        let (player_seat, joined_player_game) =
            service.join_player(&game.id, "guest-one").await.unwrap();
        assert_eq!(player_seat, 1);
        assert!(joined_player_game.players[0].connected);

        service.leave_connection(&game.id, Some(1)).await;
        let (reconnected_seat, _) = service.join_player(&game.id, "guest-one").await.unwrap();
        assert_eq!(reconnected_seat, 1);

        service.leave_connection(&game.id, Some(1)).await;
        let (replacement_seat, _) = service.join_player(&game.id, "guest-two").await.unwrap();
        assert_eq!(replacement_seat, 1);

        let spectator_game = service.join_spectator(&game.id).await.unwrap();
        assert_eq!(spectator_game.spectators, 1);

        service.leave_connection(&game.id, Some(1)).await;
        let game_after_leave = service.get_game(&game.id).await.unwrap();
        assert!(!game_after_leave.players[0].connected);
    }

    #[tokio::test]
    async fn lists_all_created_games() {
        let games: Games = Arc::new(Mutex::new(HashMap::new()));
        let service = GameService::new(games.clone());

        let first_game = service
            .create_game(CreateGame {
                name: "game a / 東京".to_string(),
                deck_size: 20,
                initial_chips: [100, 200],
            })
            .await
            .unwrap();
        let second_game = service
            .create_game(CreateGame {
                name: "game b".to_string(),
                deck_size: 30,
                initial_chips: [50, 150],
            })
            .await
            .unwrap();

        let games = service.list_games().await;
        assert_eq!(games.len(), 2);
        assert!(games.iter().any(|game| game.name == "game a / 東京"));
        assert!(games.iter().any(|game| game.name == "game b"));
        assert_ne!(first_game.id, second_game.id);
    }
}
