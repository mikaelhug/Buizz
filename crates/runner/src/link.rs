use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use buizz_core::protocol::{
    self, Ack, BarBuffer, BotMessage, Data, DataResult, End, EndReason, ErrorCode, Hello,
    PROTOCOL_VERSION, ProtocolError, Query, QueryRequest, Ready, SpineMessage, Step,
};
use buizz_core::time::Timestamp;
use buizz_market::{HistoryRequest, MarketStore};

pub(crate) const ACK_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_HISTORY_BARS: u32 = 10_000;

#[derive(Debug, thiserror::Error)]
pub(crate) enum LinkError {
    #[error("the bot closed its connection")]
    Closed,
    #[error("the bot sent no frame within {} s", ACK_TIMEOUT.as_secs())]
    Timeout,
    #[error("{1}")]
    Violation(ErrorCode, String),
}

pub(crate) struct BotLink {
    socket: WebSocket,
}

impl BotLink {
    pub(crate) async fn handshake(socket: WebSocket, ready: Ready) -> Result<Self, LinkError> {
        let mut link = Self { socket };
        match link.receive().await? {
            BotMessage::Hello(Hello { protocol, .. }) if protocol == PROTOCOL_VERSION => {}
            BotMessage::Hello(Hello { protocol, .. }) => {
                return Err(link
                    .fail(
                        ErrorCode::VersionMismatch,
                        format!("the spine speaks protocol {PROTOCOL_VERSION}, the bot {protocol}"),
                    )
                    .await);
            }
            _ => {
                return Err(link
                    .fail(ErrorCode::ProtocolViolation, "expected hello".to_owned())
                    .await);
            }
        }
        link.send(&SpineMessage::Ready(Box::new(ready))).await?;
        Ok(link)
    }

    pub(crate) async fn step(
        &mut self,
        step: Step,
        market: &MarketStore,
    ) -> Result<Ack, LinkError> {
        let (seq, clock) = (step.seq, step.clock);
        self.send(&SpineMessage::Step(Box::new(step))).await?;
        loop {
            match self.receive().await? {
                BotMessage::Query(Query { id, request }) => {
                    let answer = match answer(market, request, clock).await {
                        Ok(result) => SpineMessage::Data(Data { id, result }),
                        Err(message) => SpineMessage::Error(ProtocolError {
                            id: Some(id),
                            code: ErrorCode::BadQuery,
                            message,
                        }),
                    };
                    self.send(&answer).await?;
                }
                BotMessage::Ack(ack) if ack.seq == seq => return Ok(ack),
                BotMessage::Ack(ack) => {
                    return Err(self
                        .fail(
                            ErrorCode::ProtocolViolation,
                            format!("ack for step {} while step {seq} is open", ack.seq),
                        )
                        .await);
                }
                BotMessage::Hello(_) => {
                    return Err(self
                        .fail(ErrorCode::ProtocolViolation, "a second hello".to_owned())
                        .await);
                }
            }
        }
    }

    pub(crate) async fn end(mut self, reason: EndReason, message: Option<String>) {
        let _ = self.send(&SpineMessage::End(End { reason, message })).await;
        let _ = self.socket.send(Message::Close(None)).await;
    }

    pub(crate) async fn fail(&mut self, code: ErrorCode, message: String) -> LinkError {
        let frame = SpineMessage::Error(ProtocolError {
            id: None,
            code,
            message: message.clone(),
        });
        let _ = self.send(&frame).await;
        let _ = self.socket.send(Message::Close(None)).await;
        LinkError::Violation(code, message)
    }

    async fn send(&mut self, message: &SpineMessage) -> Result<(), LinkError> {
        let frame = protocol::encode(message)
            .map_err(|error| LinkError::Violation(ErrorCode::Internal, error.to_string()))?;
        self.socket
            .send(Message::from(frame))
            .await
            .map_err(|_| LinkError::Closed)
    }

    async fn receive(&mut self) -> Result<BotMessage, LinkError> {
        loop {
            let frame = tokio::time::timeout(ACK_TIMEOUT, self.socket.recv())
                .await
                .map_err(|_| LinkError::Timeout)?;
            match frame {
                Some(Ok(Message::Binary(bytes))) => {
                    return match protocol::decode(&bytes) {
                        Ok(message) => Ok(message),
                        Err(error) => Err(self
                            .fail(ErrorCode::ProtocolViolation, error.to_string())
                            .await),
                    };
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Text(_))) => {
                    return Err(self
                        .fail(
                            ErrorCode::ProtocolViolation,
                            "frames must be binary MessagePack".to_owned(),
                        )
                        .await);
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => return Err(LinkError::Closed),
            }
        }
    }
}

async fn answer(
    market: &MarketStore,
    request: QueryRequest,
    clock: Timestamp,
) -> Result<DataResult, String> {
    match request {
        QueryRequest::History {
            symbols,
            timeframe,
            bars,
            adjust,
        } => {
            if !(1..=MAX_HISTORY_BARS).contains(&bars) {
                return Err(format!("bars must be between 1 and {MAX_HISTORY_BARS}"));
            }
            let rows = market
                .history(HistoryRequest {
                    symbols,
                    timeframe,
                    bars,
                    adjust,
                    as_of: clock,
                })
                .await
                .map_err(|error| error.to_string())?;
            let bars = BarBuffer::from_rows(&rows).map_err(|error| error.to_string())?;
            Ok(DataResult::History { bars })
        }
        QueryRequest::Fundamentals { symbols, metrics } => {
            let points = market
                .fundamentals(symbols, metrics, clock)
                .await
                .map_err(|error| error.to_string())?;
            Ok(DataResult::Fundamentals { points })
        }
    }
}
