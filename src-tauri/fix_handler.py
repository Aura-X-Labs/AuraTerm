import sys

with open("src/ssh.rs", "r") as f:
    text = f.read()

import re

old_impl = """impl client::Handler for ClientHandler {
    type Error = russh::Error;

    fn check_server_key<'a>(
        &'a mut self,
        _server_public_key: &'a ssh_key::PublicKey,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, Self::Error>> + Send + 'a>> {
        Box::pin(async { Ok(true) })
    }
}"""

new_impl = """impl client::Handler for ClientHandler {
    type Error = russh::Error;

    fn check_server_key(
        &mut self,
        _server_public_key: &ssh_key::PublicKey,
    ) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        async { Ok(true) }
    }
}"""

if old_impl in text:
    with open("src/ssh.rs", "w") as f:
        f.write(text.replace(old_impl, new_impl))
    print("Replaced successfully!")
else:
    print("Could not find old_impl in text.")
