//! Shared UI socket client helpers.

use std::io::{self, BufReader, BufWriter};
use std::os::unix::net::UnixStream;
use std::path::Path;

use tau_proto::{
    ClientKind, EventSelector, Frame, FrameReader, FrameWriter, Hello, Message, PROTOCOL_VERSION,
    Subscribe,
};

pub(crate) type UiFrameReader = FrameReader<BufReader<UnixStream>>;
pub(crate) type UiFrameWriter = FrameWriter<BufWriter<UnixStream>>;

pub(crate) fn connect_ui_client(
    socket_path: &Path,
    client_name: impl Into<tau_proto::ExtensionName>,
) -> io::Result<(UiFrameReader, UiFrameWriter)> {
    let stream = UnixStream::connect(socket_path)?;
    let read_stream = stream.try_clone()?;
    let mut writer = FrameWriter::new(BufWriter::new(stream));
    send_hello(&mut writer, client_name)?;
    let reader = FrameReader::new(BufReader::new(read_stream));
    Ok((reader, writer))
}

pub(crate) fn connect_ui_writer(
    socket_path: &Path,
    client_name: impl Into<tau_proto::ExtensionName>,
) -> io::Result<UiFrameWriter> {
    let stream = UnixStream::connect(socket_path)?;
    let mut writer = FrameWriter::new(BufWriter::new(stream));
    send_hello(&mut writer, client_name)?;
    Ok(writer)
}

pub(crate) fn hello_frame(client_name: impl Into<tau_proto::ExtensionName>) -> Frame {
    Frame::Message(Message::Hello(Hello {
        protocol_version: PROTOCOL_VERSION,
        client_name: client_name.into(),
        client_kind: ClientKind::Ui,
    }))
}

pub(crate) fn chat_subscription_selectors() -> Vec<EventSelector> {
    vec![
        EventSelector::Prefix("ui.".to_owned()),
        EventSelector::Prefix("action.".to_owned()),
        EventSelector::Prefix("agent.".to_owned()),
        EventSelector::Prefix("session.".to_owned()),
        EventSelector::Prefix("provider.".to_owned()),
        EventSelector::Prefix("tool.".to_owned()),
        EventSelector::Prefix("extension.".to_owned()),
        EventSelector::Prefix("harness.".to_owned()),
        EventSelector::Prefix("shell.".to_owned()),
        EventSelector::Prefix("term.".to_owned()),
    ]
}

pub(crate) fn subscribe_frame(selectors: Vec<EventSelector>) -> Frame {
    Frame::Message(Message::Subscribe(Subscribe { selectors }))
}

pub(crate) fn send_hello(
    writer: &mut UiFrameWriter,
    client_name: impl Into<tau_proto::ExtensionName>,
) -> io::Result<()> {
    send_frame(writer, &hello_frame(client_name))
}

pub(crate) fn subscribe(
    writer: &mut UiFrameWriter,
    selectors: Vec<EventSelector>,
) -> io::Result<()> {
    send_frame(writer, &subscribe_frame(selectors))
}

pub(crate) fn send_frame(writer: &mut UiFrameWriter, frame: &Frame) -> io::Result<()> {
    writer.write_frame(frame).map_err(io::Error::other)?;
    writer.flush()
}
