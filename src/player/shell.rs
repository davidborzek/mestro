//! The imperative shell around the player state machine: the shared
//! slot the threads meet in and the channel the UI listens on.

use crate::player::state::{Live, PlayerUpdate};
use std::sync::{Arc, Mutex};

pub type Shared = Arc<Mutex<Option<Live>>>;

pub type UpdateTx = tokio::sync::mpsc::UnboundedSender<PlayerUpdate>;
