//! the WS connection: the message loop over the JSON protocol and
//! the replay stream that feeds the buffered request head back
//! into the handshake parser.

use super::dispatch;
use super::Bridge;
use super::WebInfo;
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
) -> Result<(), String> {
    loop {
        match ws.next().await {
            None => break,
            Some(Err(e)) => return Err(format!("ws: {e}")),
            Some(Ok(Message::Text(text))) => {
                let response = dispatch(&text, info, bridge).await;
                ws.send(Message::Text(response.into()))
                    .await
                    .map_err(|e| format!("ws send: {e}"))?;
            }
            Some(Ok(Message::Ping(data))) => {
                ws.send(Message::Pong(data))
                    .await
                    .map_err(|e| format!("ws send: {e}"))?;
            }
            Some(Ok(Message::Close(_))) | Some(Ok(Message::Frame(_))) => break,
            Some(Ok(_)) => {}
        }
    }
    Ok(())
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
