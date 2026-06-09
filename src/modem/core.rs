use chrono::{Local, Timelike};
use log::{debug, error, info};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use crate::api::SseManager;
use crate::config::SmsStorage;
use crate::db::{Contact, ModemSMS, SimCard, Sms};
use crate::decode::parse_pdu_sms;
use crate::webhook;

use super::pdu::{build_pdus, string_to_ucs2_pub};
use super::types::*;

const TERMINATORS: &[&[u8]] = &[
    b"\r\nOK\r\n",
    b"\r\nERROR\r\n",
    b"\r\n> ",
    b"\r\n+CME ERROR",
    b"\r\n+CMS ERROR",
];

const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(500);
const MAX_LINE_BUF: usize = 64 * 1024;

/// Shared routing state between the background reader task and the command processor.
/// When `response_tx` is `Some`, bytes from the serial port are buffered for the command.
/// When `raw_response_tx` is `Some`, raw bytes are buffered (for binary transfers like AT+QFDWL).
/// When both are `None`, complete lines are dispatched as URCs via `urc_tx`.
struct ReaderState {
    response_tx: Option<tokio::sync::oneshot::Sender<io::Result<String>>>,
    raw_response_tx: Option<tokio::sync::oneshot::Sender<io::Result<Vec<u8>>>>,
}

pub struct Modem {
    pub name: String,
    pub com_port: String,
    pub baud_rate: u32,
    /// The HashMap key used when this modem has no SIM (e.g. "fallback_sim_3")
    pub fallback_key: String,
    command_tx: mpsc::UnboundedSender<ATCommand>,
    pub sim_id: RwLock<Option<String>>,
    _connection_state: Arc<RwLock<ConnectionState>>,
    /// Write half of the serial stream. `None` while disconnected.
    write_half: Arc<Mutex<Option<WriteHalf<SerialStream>>>>,
    /// Shared routing state for the background reader task.
    reader_state: Arc<Mutex<ReaderState>>,
    /// URC sender (background reader holds a clone; kept here for test injection).
    _urc_tx: mpsc::UnboundedSender<String>,
    /// URC receiver — subscribed to by the ModemManager URC handler task.
    pub urc_rx: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    /// Call ID of the active outbound call, set by manager after ATD succeeds.
    pub outbound_call_id: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl Modem {
    pub async fn new(com_port: &str, baud_rate: u32, name: &str, index: usize) -> io::Result<Self> {
        let serial_stream = Self::create_serial_connection(com_port, baud_rate).await?;
        let (read_half, write_half_stream) = tokio::io::split(serial_stream);

        let (command_tx, command_rx) = mpsc::unbounded_channel::<ATCommand>();
        let (urc_tx, urc_rx) = mpsc::unbounded_channel::<String>();

        let write_half = Arc::new(Mutex::new(Some(write_half_stream)));
        let connection_state = Arc::new(RwLock::new(ConnectionState::Connected));
        let reader_state = Arc::new(Mutex::new(ReaderState { response_tx: None, raw_response_tx: None }));

        // Spawn background reader task — owns ReadHalf, routes bytes to commands or URC channel
        tokio::spawn({
            let reader_state = reader_state.clone();
            let urc_tx = urc_tx.clone();
            let write_half = write_half.clone();
            let connection_state = connection_state.clone();
            let name_c = name.to_string();
            let com_port_c = com_port.to_string();
            async move {
                Self::reader_task_main(
                    read_half, reader_state, urc_tx, write_half,
                    name_c, com_port_c, baud_rate, connection_state,
                )
                .await;
            }
        });

        // Spawn sequential command processor
        tokio::spawn({
            let write_half = write_half.clone();
            let reader_state = reader_state.clone();
            let name_c = name.to_string();
            async move {
                Self::command_processor(command_rx, write_half, reader_state, name_c).await;
            }
        });

        info!("device:{}, com:{} connected successfully", name, com_port);

        Ok(Modem {
            name: name.to_string(),
            com_port: com_port.to_string(),
            baud_rate,
            fallback_key: format!("fallback_sim_{}", index),
            command_tx,
            sim_id: RwLock::new(None),
            _connection_state: connection_state,
            write_half,
            reader_state,
            _urc_tx: urc_tx,
            urc_rx: Arc::new(Mutex::new(urc_rx)),
            outbound_call_id: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    async fn create_serial_connection(
        com_port: &str,
        baud_rate: u32,
    ) -> tokio_serial::Result<SerialStream> {
        tokio_serial::new(com_port, baud_rate)
            .timeout(Duration::from_secs(10))
            .flow_control(tokio_serial::FlowControl::None)
            .open_native_async()
    }

    // ─── Background reader task ────────────────────────────────────────────────

    /// Long-lived task that owns the read half of the serial stream.
    /// Routes bytes: when `response_tx` is set → command-response buffer;
    ///               otherwise → URC channel, line by line.
    async fn reader_task_main(
        mut read_half: ReadHalf<SerialStream>,
        reader_state: Arc<Mutex<ReaderState>>,
        urc_tx: mpsc::UnboundedSender<String>,
        write_half: Arc<Mutex<Option<WriteHalf<SerialStream>>>>,
        name: String,
        com_port: String,
        baud_rate: u32,
        connection_state: Arc<RwLock<ConnectionState>>,
    ) {
        let mut raw_buf = [0u8; 256];
        let mut line_buf: Vec<u8> = Vec::with_capacity(4096);
        loop {
            match read_half.read(&mut raw_buf).await {
                Ok(0) => {
                    error!("Reader [{}]: EOF on serial port", name);
                    let ok = Self::reader_do_reconnect(
                        &mut read_half, &write_half, &reader_state,
                        &com_port, baud_rate, &connection_state, &name,
                    )
                    .await;
                    line_buf.clear();
                    if !ok { break; }
                }
                Ok(n) => {
                    line_buf.extend_from_slice(&raw_buf[..n]);
                    if line_buf.len() > MAX_LINE_BUF {
                        error!("Reader [{}]: buffer overflow, clearing", name);
                        let mut state = reader_state.lock().await;
                        if let Some(tx) = state.response_tx.take() {
                            let _ = tx.send(Err(io::Error::other("Buffer overflow")));
                        }
                        line_buf.clear();
                        continue;
                    }
                    Self::drain_reader_buf(&mut line_buf, &reader_state, &urc_tx, &name).await;
                }
                Err(e) => {
                    error!("Reader [{}]: read error: {}", name, e);
                    let ok = Self::reader_do_reconnect(
                        &mut read_half, &write_half, &reader_state,
                        &com_port, baud_rate, &connection_state, &name,
                    )
                    .await;
                    line_buf.clear();
                    if !ok { break; }
                }
            }
        }
        error!("Reader task [{}] exiting", name);
    }

    /// Reconnect: notify any pending command, reset state, attempt up to 3 reconnects.
    /// Returns `true` on success, `false` after all attempts fail.
    async fn reader_do_reconnect(
        read_half: &mut ReadHalf<SerialStream>,
        write_half: &Arc<Mutex<Option<WriteHalf<SerialStream>>>>,
        reader_state: &Arc<Mutex<ReaderState>>,
        com_port: &str,
        baud_rate: u32,
        connection_state: &Arc<RwLock<ConnectionState>>,
        name: &str,
    ) -> bool {
        {
            let mut state = reader_state.lock().await;
            if let Some(tx) = state.response_tx.take() {
                let _ = tx.send(Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "Serial port disconnected",
                )));
            }
        }
        *connection_state.write().await = ConnectionState::Disconnected;
        *write_half.lock().await = None;

        info!("Attempting to reconnect {} on {}", name, com_port);
        for attempt in 1..=3u32 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match Self::create_serial_connection(com_port, baud_rate).await {
                Ok(new_stream) => {
                    let (new_read, new_write) = tokio::io::split(new_stream);
                    *write_half.lock().await = Some(new_write);
                    *read_half = new_read;
                    *connection_state.write().await = ConnectionState::Connected;
                    info!("Reconnected {} on {}", name, com_port);
                    return true;
                }
                Err(e) => {
                    error!("Reconnect attempt {} failed for {}: {}", attempt, name, e);
                }
            }
        }
        error!("Failed to reconnect {} after 3 attempts", name);
        false
    }

    /// Drain `line_buf`, routing its content to command response or URC channel.
    async fn drain_reader_buf(
        line_buf: &mut Vec<u8>,
        reader_state: &Arc<Mutex<ReaderState>>,
        urc_tx: &mpsc::UnboundedSender<String>,
        name: &str,
    ) {
        loop {
            let (has_str_tx, has_raw_tx) = {
                let state = reader_state.lock().await;
                (state.response_tx.is_some(), state.raw_response_tx.is_some())
            };
            if has_str_tx || has_raw_tx {
                // Command-response mode: buffer until we see a terminator
                if let Some((term, pos)) = Self::find_terminator(line_buf) {
                    let end = pos + term.len();
                    let mut state = reader_state.lock().await;
                    if let Some(tx) = state.raw_response_tx.take() {
                        // Raw bytes mode (e.g. AT+QFDWL binary download)
                        let data = line_buf[..end].to_vec();
                        drop(state);
                        line_buf.drain(..end);
                        let _ = tx.send(Ok(data));
                    } else if let Some(tx) = state.response_tx.take() {
                        let response = String::from_utf8_lossy(&line_buf[..end]).into_owned();
                        debug!("RX [{}]: {}", name, Self::format_log(&response));
                        drop(state);
                        line_buf.drain(..end);
                        let _ = tx.send(Ok(response.clone()));
                        // Some modems (e.g. EC20F) embed call-state URCs like NO CARRIER
                        // inside the command response (before OK). Re-forward them so the
                        // URC handler can update the call record.
                        Self::forward_embedded_call_urcs(&response, urc_tx, name);
                    } else {
                        drop(state);
                        line_buf.drain(..end);
                    }
                    // Loop continues: remaining bytes may be URCs
                } else {
                    break; // Need more bytes
                }
            } else {
                // URC mode: dispatch one complete \r\n-terminated line per iteration
                if let Some(pos) = line_buf.windows(2).position(|w| w == b"\r\n") {
                    let line_bytes = line_buf[..pos].to_vec();
                    line_buf.drain(..pos + 2);
                    if !line_bytes.is_empty() {
                        let line = String::from_utf8_lossy(&line_bytes).into_owned();
                        let trimmed = line.trim().to_string();
                        if !trimmed.is_empty() {
                            debug!("URC [{}]: {}", name, Self::format_log(&trimmed));
                            let _ = urc_tx.send(trimmed);
                        }
                    }
                    // Loop continues: more lines may be present
                } else {
                    break; // Need more bytes
                }
            }
        }
    }

    // ─── Command processor ─────────────────────────────────────────────────────

    /// Sequential command processor: one ATCommand at a time, with retry on transient errors.
    async fn command_processor(
        mut command_rx: mpsc::UnboundedReceiver<ATCommand>,
        write_half: Arc<Mutex<Option<WriteHalf<SerialStream>>>>,
        reader_state: Arc<Mutex<ReaderState>>,
        name: String,
    ) {
        while let Some(mut at_command) = command_rx.recv().await {
            loop {
                match Self::execute_command_impl(
                    &write_half,
                    &reader_state,
                    &at_command.command,
                    &name,
                    Duration::from_secs(8),
                )
                .await
                {
                    Ok(response) => {
                        let _ = at_command.response_tx.send(Ok(response));
                        break;
                    }
                    Err(_e) if at_command.retries < MAX_RETRIES => {
                        at_command.retries += 1;
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                    Err(e) => {
                        let _ = at_command.response_tx.send(Err(e));
                        break;
                    }
                }
            }
        }
    }

    /// Send one AT command and await its response.
    /// Write lock is released immediately after flushing, before waiting for the response,
    /// so reconnect logic is never blocked by a pending command timeout.
    async fn execute_command_impl(
        write_half: &Arc<Mutex<Option<WriteHalf<SerialStream>>>>,
        reader_state: &Arc<Mutex<ReaderState>>,
        command: &str,
        name: &str,
        timeout_dur: Duration,
    ) -> io::Result<String> {
        let (tx, rx) = tokio::sync::oneshot::channel::<io::Result<String>>();
        {
            let mut write_guard = write_half.lock().await;
            let write = write_guard.as_mut().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "Serial port not connected")
            })?;
            reader_state.lock().await.response_tx = Some(tx);
            debug!("TX [{}]: {}", name, Self::format_log(command));
            write.write_all(command.as_bytes()).await?;
            write.flush().await?;
        } // write_guard drops here, releasing the write lock before waiting for response
        match tokio::time::timeout(timeout_dur, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(io::Error::new(io::ErrorKind::BrokenPipe, "Reader channel closed")),
            Err(_) => {
                reader_state.lock().await.response_tx = None;
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("Command timeout for device {}", name),
                ))
            }
        }
    }

    /// Forward any call-state URC lines embedded inside a command response buffer.
    /// EC20F modems can include e.g. `NO CARRIER` before `OK` in the ATH response.
    fn forward_embedded_call_urcs(response: &str, urc_tx: &mpsc::UnboundedSender<String>, name: &str) {
        for line in response.split("\r\n") {
            let trimmed = line.trim();
            if matches!(trimmed, "NO CARRIER" | "BUSY" | "RING" | "CONNECT")
                || trimmed.starts_with("VOICE CALL: END")
                || trimmed.starts_with("+CLIP:")
                || trimmed.starts_with("+CEND:")
            {
                debug!("Re-forwarding embedded URC [{}]: {}", name, trimmed);
                let _ = urc_tx.send(trimmed.to_string());
            }
        }
    }

    fn find_terminator(buffer: &[u8]) -> Option<(&'static [u8], usize)> {
        TERMINATORS
            .iter()
            .filter_map(|&t| {
                buffer
                    .windows(t.len())
                    .position(|w| w == t)
                    .map(|pos| (t, pos))
            })
            .max_by_key(|&(_, pos)| pos)
    }

    pub async fn init_modem(&self, sms_storage: Option<SmsStorage>) -> io::Result<()> {
        let init_commands = vec![
            ("ATE0\r\n", "Disable echo"),
            ("AT+CMEE=1\r\n", "Enable error messages"),
            ("AT+CMGF=0\r\n", "Set PDU mode"),
            ("AT+CSCS=\"UCS2\"\r\n", "Set character encoding"),
            ("AT+CLIP=1\r\n", "Enable caller ID (CLIP)"),
        ];

        for (cmd, description) in init_commands {
            if let Err(e) = self.send_command_with_ok(cmd).await {
                error!("Failed to {}: {}", description, e);
                return Err(e);
            }
        }

        if let Some(storage) = sms_storage {
            if let Err(e) = self.configure_sms_storage(storage).await {
                log::warn!(
                    "Failed to configure SMS storage for device {} (no SIM inserted?): {}",
                    self.name,
                    e
                );
            }
        }

        if let Err(e) = self.init_sim_info().await {
            log::warn!(
                "Failed to initialize SIM info for device {}: {}",
                self.name,
                e
            );
        }

        Ok(())
    }

    async fn configure_sms_storage(&self, storage: SmsStorage) -> io::Result<()> {
        let storage_str = match storage {
            SmsStorage::SIM => "SM",
            SmsStorage::ME => "ME",
            SmsStorage::MT => "MT",
        };

        let cmd = format!("AT+CPMS=\"{0}\",\"{0}\",\"{0}\"\r\n", storage_str);

        match self.send_command_with_ok(&cmd).await {
            Ok(_) => {
                info!(
                    "SMS storage set to {} for device {}",
                    storage_str, self.name
                );
                Ok(())
            }
            Err(e) => {
                error!("Failed to set SMS storage: {}", e);
                Err(e)
            }
        }
    }

    async fn init_sim_info(&self) -> anyhow::Result<()> {
        let (iccid_result, imsi_result, phone_result) = tokio::join!(
            self.get_sim_iccid(),
            self.get_sim_imsi(),
            self.get_phone_number()
        );

        let iccid = iccid_result.ok().flatten();
        let imsi = imsi_result.ok().flatten();
        let phone_number = phone_result.ok().flatten();

        if let Some(iccid) = iccid {
            match SimCard::find_or_create_with_phone(&iccid, imsi, phone_number).await {
                Ok(_) => {
                    *self.sim_id.write().await = Some(iccid.clone());
                    info!(
                        "SIM card initialized for device {}: ICCID={}",
                        self.name, iccid
                    );
                }
                Err(e) => {
                    error!("Failed to store SIM card info: {}", e);
                    return Err(e);
                }
            }
        } else {
            log::warn!("Could not retrieve SIM card ICCID for device {}", self.name);
        }

        Ok(())
    }

    async fn send_command_priority(&self, command: &str, priority: u8) -> io::Result<String> {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let at_command = ATCommand {
            command: command.to_string(),
            response_tx,
            _priority: priority,
            retries: 0,
        };

        self.command_tx
            .send(at_command)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Command queue closed"))?;

        response_rx
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Response channel closed"))?
    }

    async fn send_command(&self, command: &str) -> io::Result<String> {
        self.send_command_priority(command, 5).await
    }

    async fn send_command_with_ok(&self, command: &str) -> io::Result<String> {
        let response = self.send_command(command).await?;

        if response.contains("OK\r\n") {
            Ok(response)
        } else {
            error!("Command failed: {}", response);
            Err(io::Error::other(format!(
                "Command failed: {}",
                Self::format_log(&response)
            )))
        }
    }

    /// Send a PDU SMS atomically: holds the write mutex across both the AT+CMGS prompt step
    /// and the PDU data step, preventing other AT commands from slipping in between and
    /// corrupting the modem's PDU input state.
    async fn send_pdu_atomic(&self, tpdu_len: usize, pdu_hex: &str) -> anyhow::Result<()> {
        let setup_cmd = format!("AT+CMGS={}\r", tpdu_len);
        let full_pdu = format!("{}\x1A", pdu_hex);

        // Hold write lock for the entire two-step PDU sequence
        let mut write_guard = self.write_half.lock().await;
        let write = write_guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Serial port not connected"))?;

        // Step 1: Send AT+CMGS=<n>\r and wait for the '>' prompt.
        let (tx1, rx1) = tokio::sync::oneshot::channel::<io::Result<String>>();
        self.reader_state.lock().await.response_tx = Some(tx1);
        debug!("TX [{}]: {}", self.name, Self::format_log(&setup_cmd));
        write.write_all(setup_cmd.as_bytes()).await?;
        write.flush().await?;
        let prompt = match tokio::time::timeout(Duration::from_secs(8), rx1).await {
            Ok(Ok(Ok(r))) => r,
            Ok(Ok(Err(e))) => return Err(e.into()),
            Ok(Err(_)) => return Err(anyhow::anyhow!("Reader channel closed")),
            Err(_) => return Err(anyhow::anyhow!("Timeout waiting for SMS prompt")),
        };
        if !prompt.contains("> ") {
            return Err(anyhow::anyhow!("SMS prompt not received: {}", Self::format_log(&prompt)));
        }

        // Step 2: Send PDU + Ctrl-Z and wait for +CMGS:/OK.
        // Use a 30-second timeout — network transmission can be slow.
        let (tx2, rx2) = tokio::sync::oneshot::channel::<io::Result<String>>();
        self.reader_state.lock().await.response_tx = Some(tx2);
        debug!("TX [{}]: <PDU {} bytes + Ctrl-Z>", self.name, pdu_hex.len() / 2);
        write.write_all(full_pdu.as_bytes()).await?;
        write.flush().await?;
        let final_response = match tokio::time::timeout(Duration::from_secs(30), rx2).await {
            Ok(Ok(Ok(r))) => r,
            Ok(Ok(Err(e))) => return Err(e.into()),
            Ok(Err(_)) => return Err(anyhow::anyhow!("Reader channel closed")),
            Err(_) => return Err(anyhow::anyhow!("Timeout waiting for SMS send confirmation")),
        };

        if final_response.contains("OK\r\n") && final_response.contains("+CMGS:") {
            Ok(())
        } else {
            error!(
                "Incomplete SMS response: {}",
                Self::format_log(&final_response)
            );
            Err(anyhow::anyhow!("Incomplete SMS response"))
        }
    }

    async fn send_sms_content<F>(
        &self,
        setup_cmd: &str,
        message: &str,
        transform_fn: F,
    ) -> anyhow::Result<String>
    where
        F: FnOnce(&str) -> anyhow::Result<String>,
    {
        let prompt_response = self.send_command_priority(setup_cmd, 1).await?;

        if !prompt_response.contains("> ") {
            return Err(anyhow::anyhow!("SMS prompt not received"));
        }

        let transformed_message = transform_fn(message)?;
        let full_message = format!("{}\x1A", transformed_message);

        let final_response = self.send_command_priority(&full_message, 1).await?;

        if final_response.contains("OK\r\n") && final_response.contains("+CMGS:") {
            Ok(final_response)
        } else {
            error!(
                "Incomplete SMS response: {}",
                Self::format_log(&final_response)
            );
            Err(anyhow::anyhow!("Incomplete SMS response"))
        }
    }

    pub async fn send_sms_pdu(
        &self,
        contact: &Contact,
        message: &str,
    ) -> anyhow::Result<(i64, String)> {
        info!("Sending SMS via PDU to {}: {}", contact.name, message);

        let sim_id = self.sim_id.read().await.clone().unwrap_or_default();

        let sms = Sms {
            id: 0,
            contact_id: contact.id.clone(),
            timestamp: Local::now().naive_local().with_nanosecond(0).unwrap(),
            message: message.to_string(),
            sim_id,
            send: true,
            status: crate::db::SmsStatus::Loading,
        };

        let sms_id = sms.insert().await?;

        match self.send_pdu_message(&contact.name, message).await {
            Ok(_) => {
                Sms::update_status_by_id(sms_id, crate::db::SmsStatus::Read).await?;
                Ok((sms_id, contact.id.clone()))
            }
            Err(e) => {
                Sms::update_status_by_id(sms_id, crate::db::SmsStatus::Failed).await?;
                Err(e)
            }
        }
    }

    async fn send_pdu_message(&self, phone: &str, message: &str) -> anyhow::Result<()> {
        let pdus = build_pdus(phone, message)?;

        for (pdu_data, tpdu_length) in pdus {
            self.send_pdu_atomic(tpdu_length, &pdu_data).await?;
        }

        Ok(())
    }

    pub async fn read_sms_async_insert(
        &self,
        _sms_type: SmsType,
        sse_manager: Arc<SseManager>,
        webhook_manager: Option<webhook::WebhookManager>,
    ) -> anyhow::Result<()> {
        // Always read ALL messages so previously-read messages aren't missed
        let sms_list = self.read_sms(SmsType::All).await?;

        if sms_list.is_empty() {
            return Ok(());
        }

        // Delete from modem FIRST so storage doesn't fill up
        if let Err(e) = self.delete_all_sms().await {
            log::warn!("Failed to delete SMS from modem after reading: {}", e);
        }

        let webhook_future = async {
            if let Some(webhook_mgr) = webhook_manager {
                for sms in &sms_list {
                    if let Err(e) = webhook_mgr.send(sms.clone()) {
                        log::error!("Failed to send webhook: {}", e);
                    }
                }
            }
        };

        let db_future = async {
            match ModemSMS::bulk_insert(&sms_list).await {
                Ok(contact_ids) => {
                    if let Ok(conversations) =
                        crate::db::Conversation::query_by_contact_ids(&contact_ids).await
                    {
                        sse_manager.send(conversations);
                    }
                }
                Err(e) => log::error!("Insert SMS error: {}", e),
            }
        };

        tokio::join!(webhook_future, db_future);
        Ok(())
    }

    pub async fn read_sms(&self, sms_type: SmsType) -> io::Result<Vec<ModemSMS>> {
        let command = format!("AT+CMGL={}\r\n", sms_type.to_at_command_pdu());
        let response = self.send_command_with_ok(&command).await?;

        let sim_id = self.sim_id.read().await.clone().unwrap_or_default();
        let trimmed = response.trim();
        if trimmed != "OK" && !trimmed.is_empty() {
            log::info!("[{}] AT+CMGL raw response: {}", sim_id, trimmed);
        }
        Ok(parse_pdu_sms(&response, &sim_id))
    }

    /// Delete all SMS messages from modem storage (AT+CMGD=1,4)
    pub async fn delete_all_sms(&self) -> io::Result<()> {
        self.send_command_with_ok("AT+CMGD=1,4\r\n").await?;
        Ok(())
    }

    async fn get_modem_info<T>(
        &self,
        command: &str,
        parser: fn(&str) -> Option<T>,
    ) -> io::Result<Option<T>> {
        let raw_response = self.send_command_with_ok(command).await?;
        let cleaned_response = raw_response.trim().replace("OK", "");
        Ok(parser(&cleaned_response))
    }

    pub async fn get_signal_quality(&self) -> io::Result<Option<SignalQuality>> {
        self.get_modem_info("AT+CSQ\r\n", SignalQuality::from_response)
            .await
    }

    pub async fn check_network_registration(
        &self,
    ) -> io::Result<Option<NetworkRegistrationStatus>> {
        // Try AT+CEREG? (EPS/LTE registration) first; fall back to AT+CREG? (CS)
        if let Ok(Some(status)) = self
            .get_modem_info("AT+CEREG?\r\n", NetworkRegistrationStatus::from_cereg_response)
            .await
        {
            return Ok(Some(status));
        }
        self.get_modem_info("AT+CREG?\r\n", NetworkRegistrationStatus::from_response)
            .await
    }

    pub async fn check_operator(&self) -> io::Result<Option<OperatorInfo>> {
        self.get_modem_info("AT+COPS?\r\n", OperatorInfo::from_response)
            .await
    }

    pub async fn get_modem_model(&self) -> io::Result<Option<ModemInfo>> {
        self.get_modem_info("AT+CGMM\r\n", ModemInfo::from_response)
            .await
    }

    pub async fn get_imei(&self) -> io::Result<Option<String>> {
        self.get_modem_info("AT+GSN\r\n", |response| {
            response
                .lines()
                .map(|l| l.trim())
                .find(|l| l.len() == 15 && l.chars().all(|c| c.is_ascii_digit()))
                .map(|s| s.to_string())
        })
        .await
    }

    pub async fn get_sim_iccid(&self) -> io::Result<Option<String>> {
        self.get_modem_info("AT+CCID\r\n", |response| {
            response
                .lines()
                .find(|line| {
                    line.contains("CCID:")
                        || (line.trim().len() >= 19
                            && line.trim().chars().all(|c| c.is_ascii_hexdigit()))
                })
                .map(|line| {
                    if line.contains("CCID:") {
                        line.split("CCID:")
                            .nth(1)
                            .map(|s| s.trim().to_string())
                            .unwrap_or_else(|| line.trim().to_string())
                    } else {
                        line.trim().to_string()
                    }
                })
        })
        .await
    }

    pub async fn get_sim_imsi(&self) -> io::Result<Option<String>> {
        self.get_modem_info("AT+CIMI\r\n", |response| {
            response
                .lines()
                .find(|line| {
                    let trimmed = line.trim();
                    trimmed.len() >= 15 && trimmed.chars().all(|c| c.is_ascii_digit())
                })
                .map(|line| line.trim().to_string())
        })
        .await
    }

    pub async fn get_phone_number(&self) -> io::Result<Option<String>> {
        // Set phonebook memory preference to SIM card before querying MSISDN.
        // Ignore errors 鈥?non-critical and modem may already be set correctly.
        let _ = self.send_command_with_ok("AT$QCPBMPREF=1\r\n").await;
        self.get_modem_info("AT+CNUM\r\n", Self::parse_phone_number)
            .await
    }

    fn parse_phone_number(response: &str) -> Option<String> {
        response
            .lines()
            .find(|line| line.starts_with("+CNUM:"))
            .and_then(|line| {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 2 {
                    let number = parts[1].trim().trim_matches('"').to_string();
                    if number.is_empty() { None } else { Some(number) }
                } else {
                    None
                }
            })
    }

    fn format_log(input: &str) -> String {
        input
            .replace("\r\n", "\\r\\n")
            .replace("\r", "\\r")
            .replace("\n", "\\n")
    }

    pub async fn send_sms_text(
        &self,
        contact: &Contact,
        message: &str,
    ) -> anyhow::Result<(i64, String)> {
        info!("Sending SMS text to {}: {}", contact.name, message);

        let sim_id = self.sim_id.read().await.clone().unwrap_or_default();

        let sms = Sms {
            id: 0,
            contact_id: contact.id.clone(),
            timestamp: Local::now().naive_local().with_nanosecond(0).unwrap(),
            message: message.to_string(),
            sim_id,
            send: true,
            status: crate::db::SmsStatus::Loading,
        };

        let sms_id = sms.insert().await?;

        match self.send_text_message(&contact.name, message).await {
            Ok(_) => {
                Sms::update_status_by_id(sms_id, crate::db::SmsStatus::Read).await?;
                Ok((sms_id, contact.id.clone()))
            }
            Err(e) => {
                Sms::update_status_by_id(sms_id, crate::db::SmsStatus::Failed).await?;
                Err(e)
            }
        }
    }

    async fn send_text_message(&self, phone: &str, message: &str) -> anyhow::Result<()> {
        self.send_sms_content(&format!("AT+CMGS=\"{}\"\r", phone), message, |msg| {
            string_to_ucs2_pub(msg)
        })
        .await?;
        Ok(())
    }

    pub async fn read_sms_sync_insert(&self, _sms_type: SmsType) -> anyhow::Result<()> {
        let sms_list = self.read_sms(SmsType::All).await?;
        if !sms_list.is_empty() {
            if let Err(e) = self.delete_all_sms().await {
                log::warn!("Failed to delete SMS from modem after reading: {}", e);
            }
            ModemSMS::bulk_insert(&sms_list).await?;
        }
        Ok(())
    }

    pub async fn get_sms_center(&self) -> io::Result<Option<String>> {
        self.get_modem_info("AT+CSCA?\r\n", |response| {
            response
                .lines()
                .find(|line| line.starts_with("+CSCA:"))
                .and_then(|line| {
                    line.split('"')
                        .nth(1)
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                })
        })
        .await
    }

    pub async fn get_network_info(&self) -> io::Result<Option<String>> {
        self.get_modem_info("AT+CPSI?\r\n", |response| {
            response
                .lines()
                .find(|line| line.starts_with("+CPSI:"))
                .map(|line| line.to_string())
        })
        .await
    }

    pub async fn get_sim_status(&self) -> io::Result<Option<String>> {
        self.get_modem_info("AT+CPIN?\r\n", |response| {
            response
                .lines()
                .find(|line| line.starts_with("+CPIN:"))
                .and_then(|line| line.split(':').nth(1).map(|s| s.trim().to_string()))
        })
        .await
    }

    pub async fn get_memory_status(&self) -> io::Result<Option<String>> {
        self.get_modem_info("AT+CPMS?\r\n", |response| {
            response
                .lines()
                .find(|line| line.starts_with("+CPMS:"))
                .map(|line| line.to_string())
        })
        .await
    }

    pub async fn get_temperature_info(&self) -> io::Result<Option<String>> {
        self.get_modem_info("AT+QTEMP?\r\n", |response| {
            response
                .lines()
                .find(|line| line.starts_with("+QTEMP:"))
                .map(|line| line.to_string())
        })
        .await
    }

    pub async fn set_sms_storage(&self, sms_storage: SmsStorage) -> io::Result<()> {
        let storage_str = match sms_storage {
            SmsStorage::SIM => "SM",
            SmsStorage::ME => "ME",
            SmsStorage::MT => "MT",
        };

        let cmd = format!("AT+CPMS=\"{0}\",\"{0}\",\"{0}\"\r\n", storage_str);

        match self.send_command_with_ok(&cmd).await {
            Ok(_) => {
                info!(
                    "SMS storage changed to {} for device {}",
                    storage_str, self.name
                );
                Ok(())
            }
            Err(e) => {
                error!(
                    "Failed to change SMS storage to {} for device {}: {}",
                    storage_str, self.name, e
                );
                Err(e)
            }
        }
    }

    pub async fn get_sms_storage_status(&self) -> io::Result<Option<String>> {
        self.get_modem_info("AT+CPMS?\r\n", |response| {
            response
                .lines()
                .find(|line| line.starts_with("+CPMS:"))
                .map(|line| line.to_string())
        })
        .await
    }

    // ─── Voice call AT commands ────────────────────────────────────────────────

    /// Initiate an outbound voice call. The trailing `;` keeps AT command mode active.
    pub async fn make_call(&self, phone: &str) -> io::Result<()> {
        self.send_command_with_ok(&format!("ATD{};\r\n", phone)).await?;
        Ok(())
    }

    /// Answer an incoming call.
    pub async fn answer_call(&self) -> io::Result<()> {
        self.send_command_with_ok("ATA\r\n").await?;
        Ok(())
    }

    /// Hang up the active or incoming call.
    pub async fn hangup_call(&self) -> io::Result<()> {
        self.send_command_with_ok("ATH\r\n").await?;
        Ok(())
    }

    /// Delete all files from modem UFS to free space before a new recording.
    pub async fn delete_files(&self) -> io::Result<()> {
        self.send_command_with_ok("AT+QFDEL=\"*\"\r\n").await?;
        Ok(())
    }

    /// Start downlink audio recording to UFS (AMR format).
    /// `filename` is stored in modem UFS, e.g. `"a.amr"`.
    pub async fn start_recording(&self, filename: &str) -> io::Result<()> {
        self.send_command_with_ok(&format!("AT+QAUDRD=1,\"{}\",3,1\r\n", filename))
            .await?;
        Ok(())
    }

    /// Stop the active audio recording.
    pub async fn stop_recording(&self) -> io::Result<()> {
        self.send_command_with_ok("AT+QAUDRD=0\r\n").await?;
        Ok(())
    }

    /// Inject a synthetic URC line into the URC channel.
    /// Used by the 30s recording timer to guarantee the URC handler processes
    /// call-end cleanup even when the modem doesn't emit NO CARRIER after ATH.
    pub fn inject_urc(&self, line: &str) {
        let _ = self._urc_tx.send(line.to_string());
    }

    /// Download a file from the modem UFS using AT+QFDWL and return its raw bytes.
    ///
    /// Protocol: modem responds `CONNECT\r\n`, then streams binary data, then
    /// `\r\n+QFDWL: <size>,<checksum>\r\nOK\r\n` when done.
    pub async fn download_file(&self, filename: &str) -> io::Result<Vec<u8>> {
        let (raw_tx, raw_rx) = tokio::sync::oneshot::channel::<io::Result<Vec<u8>>>();
        let command = format!("AT+QFDWL=\"{}\"\r\n", filename);

        {
            let mut write_guard = self.write_half.lock().await;
            let write = write_guard.as_mut().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "Serial port not connected")
            })?;
            self.reader_state.lock().await.raw_response_tx = Some(raw_tx);
            debug!("TX [{}]: {}", self.name, Self::format_log(&command));
            write.write_all(command.as_bytes()).await?;
            write.flush().await?;
            // write_guard drops here, releasing the write lock
        }

        let raw = match tokio::time::timeout(Duration::from_secs(30), raw_rx).await {
            Ok(Ok(result)) => result?,
            Ok(Err(_)) => {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "Reader channel closed"))
            }
            Err(_) => {
                self.reader_state.lock().await.raw_response_tx = None;
                return Err(io::Error::new(io::ErrorKind::TimedOut, "AT+QFDWL timeout"));
            }
        };

        Self::parse_qfdwl_response(&raw)
    }

    /// Parse the raw bytes returned by `AT+QFDWL`.
    /// Strips the `CONNECT\r\n` header and `\r\n+QFDWL: <size>,<checksum>\r\nOK\r\n` trailer.
    fn parse_qfdwl_response(raw: &[u8]) -> io::Result<Vec<u8>> {
        const CONNECT_MARKER: &[u8] = b"CONNECT\r\n";
        const TRAILER_MARKER: &[u8] = b"\r\n+QFDWL:";

        let connect_end = raw
            .windows(CONNECT_MARKER.len())
            .position(|w| w == CONNECT_MARKER)
            .map(|p| p + CONNECT_MARKER.len())
            .ok_or_else(|| io::Error::other("AT+QFDWL: missing CONNECT response"))?;

        let after_connect = &raw[connect_end..];
        let trailer_pos = after_connect
            .windows(TRAILER_MARKER.len())
            .enumerate()
            .filter(|(_, w)| *w == TRAILER_MARKER)
            .last()
            .map(|(pos, _)| pos)
            .ok_or_else(|| io::Error::other("AT+QFDWL: missing +QFDWL trailer"))?;

        Ok(after_connect[..trailer_pos].to_vec())
    }

    /// Extract the caller number from a `+CLIP:` URC line.
    /// e.g. `+CLIP: "18612345678",161,,,,0` → `Some("18612345678")`
    pub fn parse_clip(line: &str) -> Option<String> {
        if !line.starts_with("+CLIP:") {
            return None;
        }
        let rest = &line[6..];
        let start = rest.find('"')? + 1;
        let end = rest[start..].find('"')?;
        let number = &rest[start..start + end];
        if number.is_empty() { None } else { Some(number.to_string()) }
    }
}
