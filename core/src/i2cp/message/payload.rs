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
    i2cp::message::{MessageType, SessionId, I2CP_HEADER_SIZE, LOG_TARGET},
    primitives::Lease,
};

use bytes::{BufMut, BytesMut};

use alloc::vec::Vec;

/// `MessagePayload` message.
///
/// https://geti2p.net/spec/i2cp#messagepayloadmessage
pub struct MessagePayload(());

impl MessagePayload {
    /// Create new `MessagePayload` message.
    pub fn new(session_id: u16, message_id: u32, message: Vec<u8>) -> BytesMut {
        let payload_len = 2usize // session id
            + 4usize // message id
            + 4usize // payload size
            + message.len();

        let mut out = BytesMut::with_capacity(I2CP_HEADER_SIZE + payload_len);

        out.put_u32(payload_len as u32);
        out.put_u8(MessageType::MessagePayload.as_u8());
        out.put_u16(session_id);
        out.put_u32(message_id);
        out.put_u32(message.len() as u32);
        out.put_slice(&message);

        out
    }
}
