use ff_core::models::exchange::{DispatchCallRequest, DispatchCallResponse, DISPATCH_CALL_VERSION};

#[test]
fn dispatch_call_exchange_types_round_trip_json() {
    let request = DispatchCallRequest {
        request_id: "request-1".into(),
        ..Default::default()
    };
    let request_json = serde_json::to_string(&request).unwrap();
    let decoded_request: DispatchCallRequest = serde_json::from_str(&request_json).unwrap();
    assert_eq!(decoded_request, request);
    assert_eq!(decoded_request.dispatch_call_version, DISPATCH_CALL_VERSION);

    let response = DispatchCallResponse {
        response_id: "response-1".into(),
        ..Default::default()
    };
    let response_json = serde_json::to_string(&response).unwrap();
    let decoded_response: DispatchCallResponse = serde_json::from_str(&response_json).unwrap();
    assert_eq!(decoded_response, response);
    assert_eq!(
        decoded_response.dispatch_call_version,
        DISPATCH_CALL_VERSION
    );
}
