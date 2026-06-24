use super::*;

impl RegVm {
    pub(super) fn run_resource_drop(
        &mut self,
        unit: &RegUnit,
        value: VmValue,
        base: usize,
    ) -> Result<(), EvalError> {
        if self.finish_resource_pool_lease(value.clone())? {
            return Ok(());
        }
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
        let mut buffer = vec![0; max_bytes as usize];
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
        self.tcp_stream_mut(id)?
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

    pub(super) fn websocket_send(&mut self, id: i64, opcode: u8, payload: &[u8]) -> Result<(), VmValue> {
        websocket_write_frame(self.websocket_stream_mut(id)?, opcode, payload)
    }

    pub(super) fn websocket_recv(
        &mut self,
        id: i64,
        expected: WebSocketExpectedFrame,
    ) -> Result<Option<Vec<u8>>, VmValue> {
        loop {
            let frame = websocket_read_frame(self.websocket_stream_mut(id)?)?;
            match frame.opcode {
                0x1 if matches!(expected, WebSocketExpectedFrame::Text) => {
                    return Ok(Some(frame.payload));
                }
                0x2 if matches!(expected, WebSocketExpectedFrame::Binary) => {
                    return Ok(Some(frame.payload));
                }
                0x8 => return Ok(None),
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
        websocket_write_frame(self.websocket_stream_mut(id)?, 0x8, &[])
    }

    pub(super) fn resource_pool_new(
        &mut self,
        unit: &RegUnit,
        args: &[Reg],
        base: usize,
        next_base: usize,
        lazy: bool,
        factory_returns_result: bool,
    ) -> Result<VmValue, EvalError> {
        let factory = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 0)?)?;
        let max_size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
        let capacity = max_size.max(0);
        let mut idle = Vec::new();
        if !lazy {
            idle.reserve(capacity as usize);
            for _ in 0..capacity {
                let value = self.call_closure_zero(unit, &factory, next_base)?;
                if factory_returns_result {
                    match result_variant_payload(&value)? {
                        Ok(value) => idle.push(value),
                        Err(error) => return Ok(value_err(error)),
                    }
                } else {
                    idle.push(value);
                }
            }
        }
        let id = self.next_pool_id;
        self.next_pool_id = self.next_pool_id.saturating_add(1);
        self.pools.insert(
            id,
            VmResourcePool {
                capacity,
                created: idle.len() as i64,
                in_use: 0,
                idle,
                factory: lazy.then_some(factory),
                factory_returns_result,
            },
        );
        let pool = resource_pool_value(id);
        if factory_returns_result && !lazy {
            Ok(value_ok(pool))
        } else {
            Ok(pool)
        }
    }

    pub(super) fn resource_pool_borrow(
        &mut self,
        unit: &RegUnit,
        args: &[Reg],
        base: usize,
        next_base: usize,
        fallible: bool,
    ) -> Result<VmValue, EvalError> {
        let pool = expect_resource_pool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
        let borrowed = self.resource_pool_borrow_value(unit, pool.id, next_base);
        if fallible {
            return Ok(match borrowed {
                Ok(value) => value_ok(value),
                Err(error) => value_err(error),
            });
        }
        borrowed.map_err(|error| {
            EvalError::Runtime(format!(
                "ResourcePool.borrow failed: {}",
                pool_error_message(&error).unwrap_or_else(|| error.display())
            ))
        })
    }

    pub(super) fn resource_pool_borrow_value(
        &mut self,
        unit: &RegUnit,
        pool_id: i64,
        next_base: usize,
    ) -> Result<VmValue, VmValue> {
        let idle = self
            .pools
            .get_mut(&pool_id)
            .ok_or_else(|| pool_error_value(format!("unknown ResourcePool id `{pool_id}`")))?
            .idle
            .pop();
        let value = if let Some(value) = idle {
            value
        } else {
            let factory = {
                let state = self.pools.get(&pool_id).ok_or_else(|| {
                    pool_error_value(format!("unknown ResourcePool id `{pool_id}`"))
                })?;
                if state.created >= state.capacity {
                    return Err(pool_error_value("resource pool exhausted"));
                }
                state
                    .factory
                    .clone()
                    .ok_or_else(|| pool_error_value("resource pool exhausted"))?
            };
            let value = self
                .call_closure_zero(unit, &factory, next_base)
                .map_err(|error| {
                    pool_error_value(format!("resource pool factory failed: {error:?}"))
                })?;
            let factory_returns_result = self
                .pools
                .get(&pool_id)
                .map(|state| state.factory_returns_result)
                .unwrap_or(false);
            let value = if factory_returns_result {
                match result_variant_payload(&value) {
                    Ok(Ok(value)) => value,
                    Ok(Err(error)) => return Err(error),
                    Err(error) => {
                        return Err(pool_error_value(format!(
                            "resource pool factory returned non-Result value: {error:?}"
                        )));
                    }
                }
            } else {
                value
            };
            let state = self
                .pools
                .get_mut(&pool_id)
                .ok_or_else(|| pool_error_value(format!("unknown ResourcePool id `{pool_id}`")))?;
            state.created = state.created.saturating_add(1);
            value
        };
        let state = self
            .pools
            .get_mut(&pool_id)
            .ok_or_else(|| pool_error_value(format!("unknown ResourcePool id `{pool_id}`")))?;
        state.in_use = state.in_use.saturating_add(1);
        mark_pool_lease(value, pool_id).map_err(pool_error_value)
    }

    pub(super) fn finish_resource_pool_lease(&mut self, value: VmValue) -> Result<bool, EvalError> {
        let Some(lease) = split_pool_lease(value)? else {
            return Ok(false);
        };
        let state = self.pools.get_mut(&lease.pool_id).ok_or_else(|| {
            EvalError::Runtime(format!("unknown ResourcePool id `{}`.", lease.pool_id))
        })?;
        state.in_use = state.in_use.saturating_sub(1);
        if lease.discarded {
            state.created = state.created.saturating_sub(1);
        } else {
            state.idle.push(lease.value);
        }
        Ok(true)
    }
}
