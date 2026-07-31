use super::*;

impl RegVm {
    pub(super) fn authorize_intrinsic_host_access(
        &self,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
    ) -> Result<(), EvalError> {
        let Some(authority) = intrinsic.host_authority() else {
            return Ok(());
        };
        if self.execution_context.is_ambient() {
            return self.authorize_host_authority(authority);
        }

        match authority {
            crate::HostAuthority::Filesystem => {
                self.authorize_filesystem_intrinsic(intrinsic, args, base)
            }
            crate::HostAuthority::Network => {
                self.authorize_network_intrinsic(intrinsic, args, base)
            }
            crate::HostAuthority::Process => {
                self.authorize_process_intrinsic(intrinsic, args, base)
            }
            crate::HostAuthority::Environment => {
                self.authorize_environment_intrinsic(intrinsic, args, base)
            }
            crate::HostAuthority::Database
            | crate::HostAuthority::TempDirectory
            | crate::HostAuthority::Native
            | crate::HostAuthority::Jit => self.authorize_host_authority(authority),
        }
    }

    fn authorize_filesystem_intrinsic(
        &self,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
    ) -> Result<(), EvalError> {
        match intrinsic {
            RegIntrinsic::DirectoryCopyFile | RegIntrinsic::DirectoryRename => {
                self.authorize_path_arg(args, base, 0)?;
                self.authorize_path_arg(args, base, 1)
            }
            RegIntrinsic::ImageSave => self.authorize_path_arg(args, base, 1),
            RegIntrinsic::CsvReadInto
            | RegIntrinsic::FileReadAll
            | RegIntrinsic::FileReadAllString
            | RegIntrinsic::FileReadInto
            | RegIntrinsic::FileWrite
            | RegIntrinsic::FileWriteBytesView
            | RegIntrinsic::FileWriteBuffer
            | RegIntrinsic::FileWriteBufferView
            | RegIntrinsic::FileWriteString => {
                let file = expect_file_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.authorize_path(&file.path)
            }
            RegIntrinsic::TempDirKeep | RegIntrinsic::TempDirNew | RegIntrinsic::TempDirPath => {
                self.execution_context
                    .authorize_temp_directory()
                    .map_err(host_authority_error)
            }
            RegIntrinsic::ConfigLoad
            | RegIntrinsic::CsvOpenRead
            | RegIntrinsic::CsvRows
            | RegIntrinsic::DirectoryCreate
            | RegIntrinsic::DirectoryCreateAll
            | RegIntrinsic::DirectoryCreateDirAll
            | RegIntrinsic::DirectoryExists
            | RegIntrinsic::DirectoryIsDir
            | RegIntrinsic::DirectoryIsFile
            | RegIntrinsic::DirectoryListFiles
            | RegIntrinsic::DirectoryListPaths
            | RegIntrinsic::DirectoryMetadata
            | RegIntrinsic::DirectoryReadString
            | RegIntrinsic::DirectoryRemoveDirAll
            | RegIntrinsic::DirectoryRemoveFile
            | RegIntrinsic::DirectoryWriteString
            | RegIntrinsic::FileAppendBytes
            | RegIntrinsic::FileAppendString
            | RegIntrinsic::FileBytesStream
            | RegIntrinsic::FileExists
            | RegIntrinsic::FileOpen
            | RegIntrinsic::FileOpenRead
            | RegIntrinsic::FileOpenWrite
            | RegIntrinsic::FileReadAllAsync
            | RegIntrinsic::FileReadAllStringAsync
            | RegIntrinsic::FileReadBytes
            | RegIntrinsic::FileReadString
            | RegIntrinsic::FileRemove
            | RegIntrinsic::FileWriteAsync
            | RegIntrinsic::FileWriteAtomic
            | RegIntrinsic::FileWriteBytes
            | RegIntrinsic::FileWriteStringAsync
            | RegIntrinsic::FileWriteStringToPath
            | RegIntrinsic::HashSha256File
            | RegIntrinsic::ImageLoad
            | RegIntrinsic::JsonParseFile
            | RegIntrinsic::PathExists
            | RegIntrinsic::PathIsDir
            | RegIntrinsic::PathIsFile
            | RegIntrinsic::PathListFiles
            | RegIntrinsic::PathListPaths
            | RegIntrinsic::PathReadString
            | RegIntrinsic::PathWriteString
            | RegIntrinsic::RuleLoaderLoadRules
            | RegIntrinsic::TempDirNewIn
            | RegIntrinsic::TomlParseFile
            | RegIntrinsic::YamlParseFile => self.authorize_path_arg(args, base, 0),
            _ => Err(scoped_authorization_unavailable(intrinsic)),
        }
    }

    fn authorize_network_intrinsic(
        &self,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
    ) -> Result<(), EvalError> {
        match intrinsic {
            RegIntrinsic::TcpConnect => {
                let host = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let port = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let port = u16::try_from(port).map_err(|_| {
                    EvalError::Runtime(format!("network port `{port}` is outside 1..=65535"))
                })?;
                self.authorize_endpoint("tcp", host, port)
            }
            RegIntrinsic::WebSocketConnect => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let (scheme, host, port) = parse_authorized_url(url)?;
                self.authorize_endpoint(scheme, host, port)
            }
            RegIntrinsic::HttpSendAsync => {
                let request = expect_http_request_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let (scheme, host, port) = parse_authorized_url(&request.url)?;
                self.authorize_endpoint(scheme, host, port)
            }
            RegIntrinsic::HttpGet
            | RegIntrinsic::HttpGetAsync
            | RegIntrinsic::HttpGetRetryAsync
            | RegIntrinsic::HttpGetTimeoutAsync
            | RegIntrinsic::HttpPostForm
            | RegIntrinsic::HttpPostFormAsync
            | RegIntrinsic::HttpPostJson
            | RegIntrinsic::HttpPostJsonAsync
            | RegIntrinsic::HttpPostJsonBearerRetryAsync
            | RegIntrinsic::HttpPostJsonRetryAsync
            | RegIntrinsic::HttpPostJsonTimeoutAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let (scheme, host, port) = parse_authorized_url(url)?;
                self.authorize_endpoint(scheme, host, port)
            }
            RegIntrinsic::TcpStreamRead
            | RegIntrinsic::TcpStreamShutdown
            | RegIntrinsic::TcpStreamWrite
            | RegIntrinsic::TcpStreamWriteAll
            | RegIntrinsic::WebSocketClose
            | RegIntrinsic::WebSocketRecvBytes
            | RegIntrinsic::WebSocketRecvText
            | RegIntrinsic::WebSocketSendBytes
            | RegIntrinsic::WebSocketSendText => self
                .execution_context
                .authorize_host_authority(crate::HostAuthority::Network)
                .map_err(host_authority_error),
            _ => Err(scoped_authorization_unavailable(intrinsic)),
        }
    }

    fn authorize_process_intrinsic(
        &self,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
    ) -> Result<(), EvalError> {
        match intrinsic {
            RegIntrinsic::ProcessRun
            | RegIntrinsic::ProcessRunAsync
            | RegIntrinsic::ProcessRunManyStdout
            | RegIntrinsic::ProcessRunManyStdoutAsync
            | RegIntrinsic::ProcessRunManyStdoutTimeout
            | RegIntrinsic::ProcessRunManyStdoutTimeoutAsync
            | RegIntrinsic::ProcessRunStdout
            | RegIntrinsic::ProcessRunStdoutAsync
            | RegIntrinsic::ProcessRunStdoutTimeout
            | RegIntrinsic::ProcessRunStdoutTimeoutAsync
            | RegIntrinsic::ProcessRunTimeout
            | RegIntrinsic::ProcessRunTimeoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.authorize_executable(command)
            }
            RegIntrinsic::ProcessRunRequest
            | RegIntrinsic::ProcessRunRequestAsync
            | RegIntrinsic::ProcessRunRequestCancellableAsync
            | RegIntrinsic::ProcessStream => {
                let request =
                    expect_process_request_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.authorize_executable(&request.command)?;
                if let Some(cwd) = &request.cwd {
                    self.authorize_path(cwd)?;
                }
                for (name, _) in &request.env {
                    self.execution_context
                        .authorize_environment_variable(name)
                        .map_err(host_authority_error)?;
                }
                Ok(())
            }
            _ => Err(scoped_authorization_unavailable(intrinsic)),
        }
    }

    fn authorize_environment_intrinsic(
        &self,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
    ) -> Result<(), EvalError> {
        match intrinsic {
            RegIntrinsic::EnvGet | RegIntrinsic::EnvGetOrDefault | RegIntrinsic::EnvSet => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.execution_context
                    .authorize_environment_variable(name)
                    .map_err(host_authority_error)
            }
            _ => Err(scoped_authorization_unavailable(intrinsic)),
        }
    }

    fn authorize_path_arg(&self, args: &[Reg], base: usize, index: usize) -> Result<(), EvalError> {
        let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, index)?)?;
        self.authorize_path(path)
    }

    fn authorize_path(&self, path: impl AsRef<Path>) -> Result<(), EvalError> {
        let authorized = self
            .execution_context
            .authorize_filesystem_path(path)
            .map_err(host_authority_error)?;
        self.execution_context
            .host_adapters()
            .filesystem_path(&authorized)
            .map(|_| ())
            .map_err(host_authority_error)
    }

    fn authorize_endpoint(&self, scheme: &str, host: &str, port: u16) -> Result<(), EvalError> {
        let authorized = self
            .execution_context
            .authorize_network_endpoint(scheme, host, port)
            .map_err(host_authority_error)?;
        self.execution_context
            .host_adapters()
            .network_endpoint(&authorized)
            .map(|_| ())
            .map_err(host_authority_error)
    }

    fn authorize_executable(&self, executable: impl AsRef<Path>) -> Result<(), EvalError> {
        let authorized = self
            .execution_context
            .authorize_process_executable(executable)
            .map_err(host_authority_error)?;
        self.execution_context
            .host_adapters()
            .process_executable(&authorized)
            .map(|_| ())
            .map_err(host_authority_error)
    }
}

fn parse_authorized_url(url: &str) -> Result<(&str, &str, u16), EvalError> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| EvalError::Runtime(format!("network URL `{url}` has no scheme")))?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())
        .ok_or_else(|| EvalError::Runtime(format!("network URL `{url}` has no host")))?;
    if authority.contains('@') {
        return Err(EvalError::Runtime(
            "network URLs with user information are not accepted by scoped adapters".to_owned(),
        ));
    }

    let (host, explicit_port) = if let Some(ipv6) = authority.strip_prefix('[') {
        let (host, suffix) = ipv6
            .split_once(']')
            .ok_or_else(|| EvalError::Runtime(format!("network URL `{url}` has invalid IPv6")))?;
        let port = suffix.strip_prefix(':').filter(|value| !value.is_empty());
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port))
                if !host.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) =>
            {
                (host, Some(port))
            }
            _ => (authority, None),
        }
    };
    let port = match explicit_port {
        Some(port) => port
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| EvalError::Runtime(format!("network URL `{url}` has invalid port")))?,
        None => match scheme.to_ascii_lowercase().as_str() {
            "http" | "ws" => 80,
            "https" | "wss" => 443,
            _ => {
                return Err(EvalError::Runtime(format!(
                    "network URL scheme `{scheme}` has no authorized default port"
                )));
            }
        },
    };
    Ok((scheme, host, port))
}

fn host_authority_error(error: crate::AuthorityError) -> EvalError {
    EvalError::Runtime(error.to_string())
}

fn scoped_authorization_unavailable(intrinsic: RegIntrinsic) -> EvalError {
    EvalError::Runtime(format!(
        "restricted execution has no scoped host adapter for intrinsic `{intrinsic:?}`"
    ))
}

#[cfg(test)]
mod tests {
    use super::parse_authorized_url;

    #[test]
    fn parses_default_and_explicit_network_ports() {
        assert_eq!(
            parse_authorized_url("https://example.com/path").expect("https"),
            ("https", "example.com", 443)
        );
        assert_eq!(
            parse_authorized_url("ws://[::1]:9000/socket").expect("websocket"),
            ("ws", "::1", 9000)
        );
    }

    #[test]
    fn rejects_user_information_and_invalid_ports() {
        assert!(parse_authorized_url("https://user:secret@example.com").is_err());
        assert!(parse_authorized_url("http://example.com:0").is_err());
    }
}
