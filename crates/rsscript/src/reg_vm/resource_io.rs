use super::*;

impl RegVm {
    fn ensure_network_allocation(
        &mut self,
        bytes: usize,
        error: fn(String) -> VmValue,
        operation: &str,
    ) -> Result<(), VmValue> {
        if bytes > MAX_NETWORK_IO_BYTES {
            return Err(error(format!(
                "{operation} exceeds the {MAX_NETWORK_IO_BYTES}-byte limit"
            )));
        }
        self.account_bytes(bytes)
            .map_err(|limit| error(limit.into_message()))
    }

    pub(super) fn run_resource_drop(
        &mut self,
        unit: &RegUnit,
        value: VmValue,
        base: usize,
    ) -> Result<(), EvalError> {
        let VmValue::Struct(data) = value else {
            return Ok(());
        };
        let Some(function_id) = unit
            .resource_drop_functions
            .get(data.name().as_ref())
            .copied()
        else {
            return Ok(());
        };
        let callee = Rc::clone(&unit.functions[function_id]);
        self.prepare_frame(base, callee.regs)?;
        for (field, value) in data.iter() {
            if let Some(reg) = callee.local_regs.get(field.as_ref()) {
                self.set_reg(base + *reg, value.clone());
            }
        }
        let result = self.run_frame(unit, callee, base)?;
        if matches!(result, VmValue::Unit) {
            Ok(())
        } else {
            Err(EvalError::Runtime(format!(
                "resource drop for `{}` returned unsupported value `{}`.",
                data.name(),
                result.display()
            )))
        }
    }

    pub(super) fn tcp_connect(&mut self, host: &str, port: i64) -> Result<VmValue, VmValue> {
        if port <= 0 || port > u16::MAX as i64 {
            return Err(tcp_error_value("TCP port must be between 1 and 65535"));
        }
        let stream = TcpStream::connect(format!("{host}:{port}")).map_err(|error| {
            tcp_error_value(format!("TCP connect to `{host}:{port}` failed: {error}"))
        })?;
        let timeout = Some(std::time::Duration::from_secs(5));
        let _ = stream.set_read_timeout(timeout);
        let _ = stream.set_write_timeout(timeout);
        let id = self.next_tcp_stream_id;
        self.next_tcp_stream_id = self.next_tcp_stream_id.saturating_add(1);
        self.tcp_streams.insert(id, stream);
        Ok(tcp_stream_value(id))
    }

    pub(super) fn tcp_stream_mut(&mut self, id: i64) -> Result<&mut TcpStream, VmValue> {
        self.tcp_streams
            .get_mut(&id)
            .ok_or_else(|| tcp_error_value(format!("unknown TcpStream id `{id}`")))
    }

    pub(super) fn tcp_stream_read(&mut self, id: i64, max_bytes: i64) -> Result<Vec<u8>, VmValue> {
        if max_bytes <= 0 {
            return Err(tcp_error_value("TCP read max_bytes must be positive"));
        }
        let max_bytes = usize::try_from(max_bytes)
            .map_err(|_| tcp_error_value("TCP read max_bytes is too large"))?;
        self.ensure_network_allocation(max_bytes, tcp_error_value, "TCP read max_bytes")?;
        if !self.tcp_streams.contains_key(&id) {
            return Err(tcp_error_value(format!("unknown TcpStream id `{id}`")));
        }
        let mut buffer = vec![0; max_bytes];
        let read = self
            .tcp_stream_mut(id)?
            .read(&mut buffer)
            .map_err(|error| tcp_error_value(format!("TCP read failed: {error}")))?;
        buffer.truncate(read);
        Ok(buffer)
    }

    pub(super) fn tcp_stream_write(&mut self, id: i64, data: &[u8]) -> Result<i64, VmValue> {
        self.tcp_stream_mut(id)?
            .write(data)
            .map(|written| written as i64)
            .map_err(|error| tcp_error_value(format!("TCP write failed: {error}")))
    }

    pub(super) fn tcp_stream_write_all(&mut self, id: i64, data: &[u8]) -> Result<(), VmValue> {
        self.tcp_stream_mut(id)?
            .write_all(data)
            .map_err(|error| tcp_error_value(format!("TCP write_all failed: {error}")))
    }

    pub(super) fn tcp_stream_shutdown(&mut self, id: i64) -> Result<(), VmValue> {
        let stream = self
            .tcp_streams
            .remove(&id)
            .ok_or_else(|| tcp_error_value(format!("unknown TcpStream id `{id}`")))?;
        stream
            .shutdown(Shutdown::Both)
            .map_err(|error| tcp_error_value(format!("TCP shutdown failed: {error}")))
    }

    pub(super) fn websocket_connect(&mut self, url: &str) -> Result<VmValue, VmValue> {
        let (host_port, path) = parse_ws_url(url)?;
        let mut stream = TcpStream::connect(&host_port).map_err(|error| {
            websocket_error_value(format!("WebSocket connect to `{url}` failed: {error}"))
        })?;
        let timeout = Some(std::time::Duration::from_secs(5));
        let _ = stream.set_read_timeout(timeout);
        let _ = stream.set_write_timeout(timeout);
        let key = "cnNzY3JpcHQtcmVnLXZt";
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).map_err(|error| {
            websocket_error_value(format!("WebSocket handshake write failed: {error}"))
        })?;
        let mut response = Vec::new();
        let mut byte = [0; 1];
        while !response.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).map_err(|error| {
                websocket_error_value(format!("WebSocket handshake read failed: {error}"))
            })?;
            response.push(byte[0]);
            if response.len() > 8192 {
                return Err(websocket_error_value(
                    "WebSocket handshake response is too large",
                ));
            }
        }
        let response_text = String::from_utf8_lossy(&response);
        if !response_text.starts_with("HTTP/1.1 101 ")
            && !response_text.starts_with("HTTP/1.0 101 ")
        {
            return Err(websocket_error_value(format!(
                "WebSocket handshake failed: {}",
                response_text.lines().next().unwrap_or("")
            )));
        }
        let id = self.next_websocket_id;
        self.next_websocket_id = self.next_websocket_id.saturating_add(1);
        self.websockets.insert(id, stream);
        Ok(websocket_value(id))
    }

    pub(super) fn websocket_stream_mut(&mut self, id: i64) -> Result<&mut TcpStream, VmValue> {
        self.websockets
            .get_mut(&id)
            .ok_or_else(|| websocket_error_value(format!("unknown WebSocket id `{id}`")))
    }

    pub(super) fn websocket_send(
        &mut self,
        id: i64,
        opcode: u8,
        payload: &[u8],
    ) -> Result<(), VmValue> {
        websocket_write_frame(self.websocket_stream_mut(id)?, opcode, payload)
    }

    pub(super) fn websocket_recv(
        &mut self,
        id: i64,
        expected: WebSocketExpectedFrame,
    ) -> Result<Option<Vec<u8>>, VmValue> {
        loop {
            let header = websocket_read_frame_header(self.websocket_stream_mut(id)?)?;
            let len = usize::try_from(header.len)
                .map_err(|_| websocket_error_value("WebSocket frame payload is too large"))?;
            self.ensure_network_allocation(len, websocket_error_value, "WebSocket frame payload")?;
            let frame = websocket_read_frame_payload(self.websocket_stream_mut(id)?, header, len)?;
            match frame.opcode {
                0x1 if matches!(expected, WebSocketExpectedFrame::Text) => {
                    return Ok(Some(frame.payload));
                }
                0x2 if matches!(expected, WebSocketExpectedFrame::Binary) => {
                    return Ok(Some(frame.payload));
                }
                0x8 => {
                    self.websockets.remove(&id);
                    return Ok(None);
                }
                0x9 => {
                    websocket_write_frame(self.websocket_stream_mut(id)?, 0xA, &frame.payload)?;
                }
                0xA => {}
                0x1 => {
                    return Err(websocket_error_value(
                        "WebSocket received text frame while waiting for bytes",
                    ));
                }
                0x2 => {
                    return Err(websocket_error_value(
                        "WebSocket received binary frame while waiting for text",
                    ));
                }
                opcode => {
                    return Err(websocket_error_value(format!(
                        "WebSocket received unsupported opcode {opcode}"
                    )));
                }
            }
        }
    }

    pub(super) fn websocket_close(&mut self, id: i64) -> Result<(), VmValue> {
        let mut stream = self
            .websockets
            .remove(&id)
            .ok_or_else(|| websocket_error_value(format!("unknown WebSocket id `{id}`")))?;
        let write_result = websocket_write_frame(&mut stream, 0x8, &[]);
        let shutdown_result = stream.shutdown(Shutdown::Both);
        write_result?;
        shutdown_result
            .map_err(|error| websocket_error_value(format!("WebSocket shutdown failed: {error}")))
    }
}
