package maya2c

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func serve(t *testing.T, status int, body string) *Client {
	t.Helper()
	s := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(status)
		_, _ = w.Write([]byte(body))
	}))
	t.Cleanup(s.Close)
	return NewClient(s.URL)
}

// A proxy's 502 or the node's 429 is reported as that status, not as a
// confusing JSON decode error or, with a JSON body, as a result.
func TestANonSuccessStatusIsAnRPCError(t *testing.T) {
	for _, status := range []int{http.StatusTooManyRequests, http.StatusBadGateway} {
		var out int
		err := serve(t, status, `{"result":7}`).Call("x", nil, &out)
		var rpc *RPCError
		if !errors.As(err, &rpc) || !strings.Contains(rpc.Detail, http.StatusText(status)) {
			t.Fatalf("status %d: got %v (out %d)", status, err, out)
		}
	}
}

// jsonrpsee answers an oversized or malformed request with 413 or 400 and a
// JSON-RPC error body; the node's message must reach the caller.
func TestANonSuccessStatusKeepsTheNodesJSONRPCError(t *testing.T) {
	body := `{"jsonrpc":"2.0","id":null,"error":{"code":-32007,"message":"Request is too big"}}`
	err := serve(t, http.StatusRequestEntityTooLarge, body).Call("x", nil, nil)
	var rpc *RPCError
	if !errors.As(err, &rpc) || !strings.Contains(rpc.Detail, "413") || !strings.Contains(rpc.Detail, "Request is too big") {
		t.Fatalf("got %v", err)
	}
}

// The body is decoded into memory; a hostile endpoint must not exhaust it.
func TestAReplyLargerThanTheCapIsAnRPCError(t *testing.T) {
	var out string
	err := serve(t, http.StatusOK, `{"result":"`+strings.Repeat("a", MaxReplyBytes)+`"}`).Call("x", nil, &out)
	var rpc *RPCError
	if !errors.As(err, &rpc) || !strings.Contains(rpc.Detail, "larger than") {
		t.Fatalf("got %v (out %d bytes)", err, len(out))
	}
}

func TestAWellFormedReplyStillDecodes(t *testing.T) {
	var out int
	if err := serve(t, http.StatusOK, `{"jsonrpc":"2.0","id":1,"result":7}`).Call("x", nil, &out); err != nil || out != 7 {
		t.Fatalf("got %d, %v", out, err)
	}
}
