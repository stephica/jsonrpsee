use jsonrpsee::{core::RpcResult, proc_macros::rpc};

// Subscription method name conflict with notif override.
#[rpc(client, server)]
pub trait DupName {
	#[subscription(name = "one" => "one", unsubscribe = "unsubscribeOne", item = u8)]
	fn one(&self) -> RpcResult<()>;
}

fn main() {}
