#![no_main]

use arbitrary::Arbitrary;
use emissary_core::i2np::{Message, MessageBuilder, MessageType};
use libfuzzer_sys::fuzz_target;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Arbitrary)]
enum GeneratedMessageType {
    DatabaseStore,
    DatabaseLookup,
    DatabaseSearchReply,
    DeliveryStatus,
    Garlic,
    TunnelData,
    TunnelGateway,
    Data,
    TunnelBuild,
    TunnelBuildReply,
    VariableTunnelBuild,
    VariableTunnelBuildReply,
    ShortTunnelBuild,
    OutboundTunnelBuildReply,
}

impl Into<MessageType> for GeneratedMessageType {
    fn into(self) -> MessageType {
        match self {
            GeneratedMessageType::DatabaseStore => MessageType::DatabaseStore,
            GeneratedMessageType::DatabaseLookup => MessageType::DatabaseLookup,
            GeneratedMessageType::DatabaseSearchReply => MessageType::DatabaseSearchReply,
            GeneratedMessageType::DeliveryStatus => MessageType::DeliveryStatus,
            GeneratedMessageType::Garlic => MessageType::Garlic,
            GeneratedMessageType::TunnelData => MessageType::TunnelData,
            GeneratedMessageType::TunnelGateway => MessageType::TunnelGateway,
            GeneratedMessageType::Data => MessageType::Data,
            GeneratedMessageType::TunnelBuild => MessageType::TunnelBuild,
            GeneratedMessageType::TunnelBuildReply => MessageType::TunnelBuildReply,
            GeneratedMessageType::VariableTunnelBuild => MessageType::VariableTunnelBuild,
            GeneratedMessageType::VariableTunnelBuildReply => MessageType::VariableTunnelBuildReply,
            GeneratedMessageType::ShortTunnelBuild => MessageType::ShortTunnelBuild,
            GeneratedMessageType::OutboundTunnelBuildReply => MessageType::OutboundTunnelBuildReply,
        }
    }
}

fuzz_target!(|buffer: Vec<(GeneratedMessageType, u32, u64, Vec<u8>)>| {
    for values in buffer {
        let standard = {
            let message = MessageBuilder::standard()
                .with_message_type(values.0.into())
                .with_message_id(values.1)
                .with_expiration(Duration::from_secs(values.2))
                .with_payload(&values.3)
                .build();

            // verify that two different ways of serializing results in the message
            if !values.3.is_empty() {
                let message2 = Message {
                    message_type: values.0.into(),
                    message_id: values.1,
                    expiration: Duration::from_secs(values.2),
                    payload: values.3.clone(),
                }
                .serialize_standard();

                assert_eq!(message, message2);
            }

            Message::parse_standard(&message)
        };

        let short = {
            let message = MessageBuilder::short()
                .with_message_type(values.0.into())
                .with_message_id(values.1)
                .with_expiration(Duration::from_secs(values.2))
                .with_payload(&values.3)
                .build();

            // verify that two different ways of serializing results in the message
            if !values.3.is_empty() {
                let message2 = Message {
                    message_type: values.0.into(),
                    message_id: values.1,
                    expiration: Duration::from_secs(values.2),
                    payload: values.3.clone(),
                }
                .serialize_short();

                assert_eq!(message, message2);
            }

            Message::parse_short(&message)
        };

        if values.3.is_empty() {
            assert!(standard.is_none());
            assert!(short.is_none());
        } else {
            let standard = standard.unwrap();
            let short = short.unwrap();

            assert_eq!(standard.message_id, short.message_id);
            assert_eq!(standard.message_type, short.message_type);
            assert_eq!(standard.payload, short.payload);
        }
    }
});
