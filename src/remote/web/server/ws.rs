//! the WS connection: the message loop over the JSON protocol,
//! the replay stream that feeds the buffered request head back
//! into the handshake parser, and the push side. Every PAIRED
//! socket starts with the latest state snapshot and then follows
//! the broadcast (a lagged receiver resends the snapshot instead
//! of replaying a burst).
//!
//! Unpaired sockets (AP-mode setup clients) get none of that: no
//! snapshot, no pushes — dispatch answers hello + wifi ops and
//! nothing else. When AP mode ends their trust basis is gone and
//! the socket closes.

use super::dispatch;
use super::{Bridge, Ingress, WebInfo, WifiChannel};
use crate::remote::web::Events;
use futures_util::{SinkExt, StreamExt};
use std::io::Cursor;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

pub(super) async fn ws_loop(
    mut ws: tokio_tungstenite::WebSocketStream<Prefixed>,
    info: &WebInfo,
    bridge: &Bridge,
    events: &Events,
    wifi: &WifiChannel,
    ingress: Ingress,
    paired: bool,
) -> Result<(), String> {
    // subscribe before reading the snapshot: a publish in between
    // then arrives as a push instead of being lost. Unpaired
    // sockets never subscribe — player data is paired-only
    let mut pushes = paired.then(|| events.subscribe());
    if paired {
        if let Some(snapshot) = events.snapshot() {
            ws.send(Message::Text(snapshot.into()))
                .await
                .map_err(|e| format!("ws send: {e}"))?;
        }
    }
    // an unpaired socket lives only as long as AP mode does
    let mut ap_watch = tokio::time::interval(std::time::Duration::from_secs(1));
    ap_watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // the futures are dropped before the handlers run, so
        // both sides may use `ws`
        enum Step {
            Socket(Option<Result<Message, tokio_tungstenite::tungstenite::Error>>),
            Push(Result<String, tokio::sync::broadcast::error::RecvError>),
            ApWatch,
        }
        let step = tokio::select! {
            incoming = ws.next() => Step::Socket(incoming),
            push = async {
                match pushes.as_mut() {
                    Some(pushes) => pushes.recv().await,
                    // paired-only: never resolves without a subscription
                    None => std::future::pending().await,
                }
            } => Step::Push(push),
            _ = ap_watch.tick(), if !paired => Step::ApWatch,
        };
        match step {
            // AP mode ended: the trust basis of an unpaired socket
            // is gone — close it (the phone pairs again)
            Step::ApWatch if !info.ap() => break,
            Step::ApWatch => {}
            Step::Push(Ok(frame)) => {
                ws.send(Message::Text(frame.into()))
                    .await
                    .map_err(|e| format!("ws send: {e}"))?;
            }
            Step::Push(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                // too slow for the burst: jump to the latest, on a
                // fresh receiver so the stale backlog is not replayed
                pushes = paired.then(|| events.subscribe());
                if let Some(snapshot) = events.snapshot() {
                    ws.send(Message::Text(snapshot.into()))
                        .await
                        .map_err(|e| format!("ws send: {e}"))?;
                }
            }
            Step::Push(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
            Step::Socket(None) => break,
            Step::Socket(Some(Err(e))) => return Err(format!("ws: {e}")),
            Step::Socket(Some(Ok(Message::Text(text)))) => {
                // an unpaired socket must not outlive AP mode even
                // mid-op: the watch races the dispatch, a long
                // scan/join cannot delay the close
                let outcome = tokio::select! {
                    response = dispatch(&text, info, bridge, wifi, ingress, paired) => Ok(response),
                    true = ap_over(info), if !paired => Err(()),
                };
                match outcome {
                    Ok(response) => {
                        ws.send(Message::Text(response.into()))
                            .await
                            .map_err(|e| format!("ws send: {e}"))?;
                    }
                    // AP mode ended mid-op: close without a reply
                    Err(()) => break,
                }
            }
            Step::Socket(Some(Ok(Message::Ping(data)))) => {
                ws.send(Message::Pong(data))
                    .await
                    .map_err(|e| format!("ws send: {e}"))?;
            }
            Step::Socket(Some(Ok(Message::Close(_))))
            | Step::Socket(Some(Ok(Message::Frame(_)))) => {
                break;
            }
            Step::Socket(Some(Ok(_))) => {}
        }
    }
    Ok(())
}

/// resolves once AP mode has ended — the unpaired sockets' expiry
/// watch, polled while ops run too
async fn ap_over(info: &WebInfo) -> bool {
    loop {
        if !info.ap() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// a stream that first yields a buffered prefix (the request head
/// consumed for routing), then the socket itself
pub(super) struct Prefixed {
    pub(super) prefix: Cursor<Vec<u8>>,
    pub(super) inner: TcpStream,
}

impl AsyncRead for Prefixed {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let unfilled = buf.initialize_unfilled();
        let n = std::io::Read::read(&mut this.prefix, unfilled)?;
        if n > 0 {
            buf.advance(n);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Prefixed {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
