// Implementation note: hyper's API is not adapted to async/await at all, and there's
// unfortunately a lot of boilerplate here that could be removed once/if it gets reworked.
//
// Additionally, despite the fact that hyper is capable of performing requests to multiple different
// servers through the same `hyper::Client`, we don't use that feature on purpose. The reason is
// that we need to be guaranteed that hyper doesn't re-use an existing connection if we ever reset
// the JSON-RPC request id to a value that might have already been used.

use futures::prelude::*;
use jsonrpsee_types::{error::GenericTransportError, http::HttpConfig, jsonrpc};
use surf::http::{mime::JSON, Url};
use thiserror::Error;

const CONTENT_TYPE_JSON: &str = "application/json";

/// HTTP Transport Client.
#[derive(Debug, Clone)]
pub struct HttpTransportClient {
	/// Target to connect to.
	target: Url,
	/// HTTP client,
	client: surf::Client,
	/// Configurable max request body size
	config: HttpConfig,
}

impl HttpTransportClient {
	/// Initializes a new HTTP client.
	pub fn new(target: impl AsRef<str>, config: HttpConfig) -> Result<Self, Error> {
		let target = Url::parse(target.as_ref()).map_err(|e| Error::Url(format!("Invalid URL: {}", e)))?;
		if target.scheme() == "http" {
			Ok(HttpTransportClient { client: surf::Client::new(), target, config })
		} else {
			Err(Error::Url("URL scheme not supported, expects 'http'".into()))
		}
	}

	/// Send request.
	async fn send_request(&self, request: jsonrpc::Request) -> Result<surf::Response, Error> {
		let body = jsonrpc::to_vec(&request).map_err(Error::Serialization)?;
		log::debug!("send: {}", request);

		if body.len() > self.config.max_request_body_size as usize {
			return Err(Error::RequestTooLarge);
		}

		let request =
			surf::post(&self.target).body(body).header("accept", CONTENT_TYPE_JSON).content_type(JSON).build();

		let response = self.client.send(request).await.unwrap();
		if response.status().is_success() {
			Ok(response)
		} else {
			Err(Error::RequestFailure { status_code: response.status().into() })
		}
	}

	/// Send notification.
	pub async fn send_notification(&self, request: jsonrpc::Request) -> Result<(), Error> {
		let _response = self.send_request(request).await?;
		Ok(())
	}

	/// Send request and wait for response.
	pub async fn send_request_and_wait_for_response(
		&self,
		request: jsonrpc::Request,
	) -> Result<jsonrpc::Response, Error> {
		let mut response = self.send_request(request).await.map_err(|e| Error::Http(Box::new(e)))?;

		let length = response.len().unwrap_or(0);

		if length > self.config.max_request_body_size as usize {
			return Err(Error::RequestTooLarge.into());
		}

		let mut buffer = Vec::new();
		let reader = response.take_body().into_reader();
		let mut take = reader.take(self.config.max_request_body_size as u64);
		take.read_to_end(&mut buffer).await.map_err(|e| Error::Http(Box::new(e)))?;

		let response: jsonrpc::Response = jsonrpc::from_slice(&buffer).map_err(Error::ParseError)?;
		// Note that we don't check the Content-Type of the request. This is deemed
		// unnecessary, as a parsing error while happen anyway.
		log::debug!("recv: {}", jsonrpc::to_string(&response).expect("request valid JSON; qed"));
		Ok(response)
	}
}

/// Error that can happen during a request.
#[derive(Debug, Error)]
pub enum Error {
	/// Invalid URL.
	#[error("Invalid Url: {0}")]
	Url(String),

	/// Error while serializing the request.
	// TODO: can that happen?
	#[error("Error while serializing the request")]
	Serialization(#[source] serde_json::error::Error),

	/// Response given by the server failed to decode as UTF-8.
	#[error("Response body is not UTF-8")]
	Utf8(#[source] std::string::FromUtf8Error),

	/// Error during the HTTP request, including networking errors and HTTP protocol errors.
	#[error("Error while performing the HTTP request")]
	Http(Box<dyn std::error::Error + Send + Sync>),

	/// Server returned a non-success status code.
	#[error("Server returned an error status code: {:?}", status_code)]
	RequestFailure {
		/// Status code returned by the server.
		status_code: u16,
	},

	/// Failed to parse the JSON returned by the server into a JSON-RPC response.
	#[error("Error while parsing the response body")]
	ParseError(#[source] serde_json::error::Error),

	/// Request body too large.
	#[error("The request body was too large")]
	RequestTooLarge,
}

impl<T> From<GenericTransportError<T>> for Error
where
	T: std::error::Error + Send + Sync + 'static,
{
	fn from(err: GenericTransportError<T>) -> Self {
		match err {
			GenericTransportError::<T>::TooLarge => Self::RequestTooLarge,
			GenericTransportError::<T>::Inner(e) => Self::Http(Box::new(e)),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{Error, HttpTransportClient};
	use jsonrpsee_types::{
		http::HttpConfig,
		jsonrpc::{Call, Id, MethodCall, Params, Request, Version},
	};

	#[test]
	fn invalid_http_url_rejected() {
		let err = HttpTransportClient::new("ws://localhost:9933", HttpConfig::default()).unwrap_err();
		assert!(matches!(err, Error::Url(_)));
	}

	#[tokio::test]
	async fn request_limit_works() {
		let eighty_bytes_limit = 80;
		let client =
			HttpTransportClient::new("http://localhost:9933", HttpConfig { max_request_body_size: 80 }).unwrap();
		assert_eq!(client.config.max_request_body_size, eighty_bytes_limit);

		let request = Request::Single(Call::MethodCall(MethodCall {
			jsonrpc: Version::V2,
			method: "request_larger_than_eightybytes".to_string(),
			params: Params::None,
			id: Id::Num(1),
		}));
		let bytes = serde_json::to_vec(&request).unwrap();
		assert_eq!(bytes.len(), 81);
		let response = client.send_request(request).await.unwrap_err();
		assert!(matches!(response, Error::RequestTooLarge));
	}
}