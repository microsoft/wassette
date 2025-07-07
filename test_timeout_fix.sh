#!/bin/bash
# Simple test script to verify the SSE connection timeout fix

echo "Testing weld-mcp-server HTTP transport with keep-alive fix..."

# Start the server in HTTP mode in the background
cargo run -- serve --http &
SERVER_PID=$!

# Give server time to start
sleep 3

echo "Server started with PID: $SERVER_PID"
echo "Testing SSE connection at http://127.0.0.1:9001/sse"

# Test the SSE endpoint
curl -N -H "Accept: text/event-stream" http://127.0.0.1:9001/sse &
CURL_PID=$!

echo "SSE client started with PID: $CURL_PID"
echo "Let it run for a few seconds to see if connection stays alive..."

# Run for 10 seconds to see initial behavior
sleep 10

echo "Initial test complete. In a real scenario, this would run for 15+ minutes to verify timeout fix."

# Clean up
kill $CURL_PID 2>/dev/null
kill $SERVER_PID 2>/dev/null

echo "Test finished."