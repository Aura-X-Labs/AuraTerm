import re

with open("src/ssh.rs", "r") as f:
    text = f.read()

text = text.replace("use ssh_key::PublicKey;", "")

text = re.sub(
    r"fn check_server_key\(\s*&mut self,\s*_server_public_key: &ssh_key::PublicKey,\s*\) -> impl std::future::Future<Output = Result<bool, Self::Error>> \+ Send \{\s*async \{ Ok\(true\) \}\s*\}",
    r"fn check_server_key(self: &mut Self, _server_public_key: &russh::keys::PublicKey) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {\n        async { Ok(true) }\n    }",
    text
)

text = text.replace(
    "let session_res = client::connect(config, addr, handler).await;\n    if session_res.is_err() {\n         return Err(format!(\"Connection error: {}\", session_res.unwrap_err()));\n    }",
    "let session_res = client::connect(config, addr, handler).await;\n    if let Err(e) = session_res {\n         return Err(format!(\"Connection error: {}\", e));\n    }"
)

text = text.replace("Ok(true) => {", "Ok(russh::AuthResult::Success) => {")
text = text.replace("Ok(false) => {", "Ok(russh::AuthResult::Failure { .. }) => {")

text = text.replace(
    "russh::client::KeyboardInteractiveAuthResponse::InfoRequest {\n                    name,\n                    instruction,\n                    prompts,\n                }",
    "russh::client::KeyboardInteractiveAuthResponse::InfoRequest {\n                    name,\n                    instructions,\n                    prompts,\n                }"
)

text = text.replace(
    "name: name.into_owned(),\n                        instruction: instruction.into_owned(),",
    "name: name.clone(),\n                        instruction: instructions.clone(),"
)

text = text.replace("p.prompt.clone().into_owned()", "p.prompt.clone()")

# also unused imports, let's remove warnings
text = re.sub(r"use russh::\{\s*client::\{self, Msg, Session\},\s*Channel, ChannelMsg,\s*\};", "use russh::{client, ChannelMsg};", text)
text = text.replace("use tauri::{AppHandle, Emitter, Manager, State, Window};", "use tauri::{AppHandle, Emitter, State};")
text = text.replace("use tokio::io::{AsyncReadExt, AsyncWriteExt};\n", "")
text = text.replace("use tokio::time::{sleep, Duration};\n", "")

with open("src/ssh.rs", "w") as f:
    f.write(text)
