use crate::{HotStore, WeightedEntity};
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info};

/// Async adapter that ingests a live market feed from Binance's WebSocket API
pub struct BinanceFeedAdapter {
    store: HotStore,
}

impl BinanceFeedAdapter {
    /// Create a new BinanceFeedAdapter attached to the existing HotStore
    pub fn new(store: HotStore) -> Self {
        Self { store }
    }

    /// Run the adapter, polling messages indefinitely until disconnect
    pub async fn run(&self) {
        let url = "wss://stream.binance.com:9443/ws/!ticker@arr";
        let (ws_stream, _) = match connect_async(url).await {
            Ok(s) => s,
            Err(e) => {
                error!("Binance WebSocket connect failed: {}", e);
                return;
            }
        };

        info!("BinanceFeedAdapter connected to Live WebSocket `!ticker@arr`");
        let (_, mut read) = ws_stream.split();

        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(t)) => {
                    self.process_batch(&t);
                }
                Ok(Message::Ping(_)) => {}
                Ok(Message::Pong(_)) => {}
                Ok(Message::Close(_)) => {
                    info!("Binance WebSocket disconnected");
                    break;
                }
                Err(e) => {
                    error!("Binance WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }
    }

    #[inline]
    fn process_batch(&self, json_str: &str) {
        let value: Result<Value, _> = serde_json::from_str(json_str);
        if let Ok(Value::Array(arr)) = value {
            for item in arr {
                if let (Some(sym), Some(price_str)) = (item["s"].as_str(), item["c"].as_str()) {
                    if let Ok(price) = price_str.parse::<f64>() {
                        // Deterministic ID for symbol
                        let mut hasher = DefaultHasher::new();
                        sym.hash(&mut hasher);
                        let entity_id = hasher.finish();

                        // Register if missing, then tick
                        if !self.store.contains(entity_id) {
                            self.store.insert(WeightedEntity::new(entity_id, 100.0));
                            debug!("Discovered symbol: {} -> ID {}", sym, entity_id);
                        }
                        
                        self.store.tick_price(entity_id, price.into());
                        // Apply a small synthetic weight bump to simulate market momentum
                        self.store.update_weight(entity_id, 0.05, 0.9, 0.1); 
                        
                        // Random causal spike 1% of the time to exercise Agent
                        if fastrand::f32() < 0.01 {
                            self.store.set_causal_trigger(entity_id, true);
                        }
                    }
                }
            }
        }
    }
}
