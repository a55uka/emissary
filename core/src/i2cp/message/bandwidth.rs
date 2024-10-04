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
    i2cp::message::{MessageType, I2CP_HEADER_SIZE},
    primitives::{Date, Str},
};

use bytes::{BufMut, BytesMut};

/// `BandwidthLimits` message.
///
/// https://geti2p.net/spec/i2cp#bandwidthlimitsmessage
pub struct BandwidthLimits(());

impl BandwidthLimits {
    /// Create new [`BandwidthLimits`] message.
    pub fn new() -> BytesMut {
        let mut out = BytesMut::with_capacity(I2CP_HEADER_SIZE + 16 * 4);

        out.put_u32(16 * 4);
        out.put_u8(MessageType::BandwidthLimits.as_u8());

        out.put_u32(500); // client inbound limit (KBps)
        out.put_u32(500); // client outbound limit (KBps)
        out.put_u32(2000); // router inbound limit (KBps)
        out.put_u32(2000); // router inbound burst limit (KBps)
        out.put_u32(2000); // router outbound limit (KBps)
        out.put_u32(2000); // router outbound burst limit (KBps)
        out.put_u32(5); // router burst time (seconds)

        // nine 4-byte integers, undefined
        for _ in 0..9 {
            out.put_u32(0);
        }

        out
    }
}
