// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use crate::{
    error::Error,
    runtime::{Runtime, TcpListener},
    transports::ntcp2::LOG_TARGET,
    util::is_global,
};

use futures::Stream;

use alloc::{format, string::String};
use core::{
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

// TODO: support both ipv4 and ipv6

/// NTCP2 listener.
pub struct Ntcp2Listener<R: Runtime> {
    /// Allow local addresses.
    allow_local: bool,

    /// TCP Listener.
    listener: R::TcpListener,
}

impl<R: Runtime> Ntcp2Listener<R> {
    /// Create new [`Ntcp2Listener`].
    pub async fn new(address: SocketAddr, allow_local: bool) -> crate::Result<Self> {
        let listener = match R::TcpListener::bind(address).await {
            Some(listener) => listener,
            None => {
                tracing::debug!(
                    target: LOG_TARGET,
                    ?address,
                    "failed to bind to address, binding to 0.0.0.0",
                );

                R::TcpListener::bind(
                    format!("0.0.0.0:{}", address.port()).parse().expect("to succeed"),
                )
                .await
                .ok_or(Error::Custom(String::from("failed to bind to 0.0.0.0")))?
            }
        };

        Ok(Self {
            allow_local,
            listener,
        })
    }

    /// Get local address of the TCP listener.
    #[cfg(test)]
    pub fn local_address(&self) -> SocketAddr {
        self.listener.local_address().expect("to succeed")
    }
}

impl<R: Runtime> Stream for Ntcp2Listener<R> {
    type Item = R::TcpStream;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match self.listener.poll_accept(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some((stream, address))) => match address {
                    SocketAddr::V4(address) if !is_global(*address.ip()) && !self.allow_local => {
                        tracing::warn!(
                            target: LOG_TARGET,
                            ?address,
                            "incoming connection from local address but local addresses were disabled",
                        );
                        continue;
                    }
                    _ => return Poll::Ready(Some(stream)),
                },
            }
        }
    }
}
