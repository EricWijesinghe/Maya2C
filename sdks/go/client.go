// Package maya2c is a Go client for a Maya2C node, with transfers signed by
// the Rust wallet.
//
// This package never implements the hybrid ML-DSA-65 + SLH-DSA signature or
// the transaction wire format: both are consensus rules, and a second
// implementation is a second thing that can disagree with the chain. Signing
// runs through l1-wallet, the Rust wallet shipped with the node; this package
// submits the result and reads the chain. Standard library only.
package maya2c

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"sync/atomic"
	"time"
)

// RPCError is a JSON-RPC error from the node, or a failure to reach it.
type RPCError struct {
	Method string
	Detail string
}

func (e *RPCError) Error() string { return e.Method + ": " + e.Detail }

// Client is a node's JSON-RPC endpoint, e.g. http://127.0.0.1:8545.
type Client struct {
	URL  string
	HTTP *http.Client
	ids  atomic.Uint64
}

// NewClient returns a client with a ten-second timeout.
func NewClient(url string) *Client {
	return &Client{URL: url, HTTP: &http.Client{Timeout: 10 * time.Second}}
}

// Call invokes method and decodes its result into out.
func (c *Client) Call(method string, params []any, out any) error {
	if params == nil {
		params = []any{}
	}
	body, err := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": c.ids.Add(1), "method": method, "params": params})
	if err != nil {
		return &RPCError{method, err.Error()}
	}
	resp, err := c.HTTP.Post(c.URL, "application/json", bytes.NewReader(body))
	if err != nil {
		return &RPCError{method, err.Error()}
	}
	defer resp.Body.Close()
	var reply struct {
		Result json.RawMessage `json:"result"`
		Error  json.RawMessage `json:"error"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&reply); err != nil {
		return &RPCError{method, err.Error()}
	}
	if len(reply.Error) > 0 && string(reply.Error) != "null" {
		return &RPCError{method, string(reply.Error)}
	}
	if out == nil {
		return nil
	}
	if err := json.Unmarshal(reply.Result, out); err != nil {
		return &RPCError{method, fmt.Sprintf("decoding result: %v", err)}
	}
	return nil
}

// Account is an account's state.
type Account struct {
	Address string `json:"address"`
	Balance uint64 `json:"balance"`
	Nonce   uint64 `json:"nonce"`
}

// AccountAtTip is an account read together with the tip's height and id.
type AccountAtTip struct {
	Account
	Height  uint64 `json:"height"`
	BlockID string `json:"block_id"`
}

// Balance returns an account's balance.
func (c *Client) Balance(address string) (uint64, error) {
	var a Account
	err := c.Call("get_balance", []any{address}, &a)
	return a.Balance, err
}

// AccountAtTip returns an account with the tip it was read at.
func (c *Client) AccountAtTip(address string) (AccountAtTip, error) {
	var a AccountAtTip
	err := c.Call("get_account_at_tip", []any{address}, &a)
	return a, err
}

// Block is the part of a block this client reads.
type Block struct {
	Height uint64 `json:"height"`
	Header struct {
		ID        string `json:"id"`
		PrevHash  string `json:"prev_hash"`
		StateRoot string `json:"state_root"`
	} `json:"header"`
}

// BlockByHeight returns the block at height on the node's best chain.
func (c *Client) BlockByHeight(height uint64) (Block, error) {
	var b Block
	err := c.Call("get_block_by_height", []any{height}, &b)
	return b, err
}

// SendRawTransaction submits a signed transaction and returns its txid.
func (c *Client) SendRawTransaction(rawHex string) (string, error) {
	var r struct {
		TxID string `json:"txid"`
	}
	err := c.Call("send_raw_transaction", []any{rawHex}, &r)
	return r.TxID, err
}

// WaitUntil polls check until it holds or timeout passes.
func WaitUntil(check func() bool, timeout time.Duration) (time.Duration, error) {
	start := time.Now()
	for time.Since(start) < timeout {
		if check() {
			return time.Since(start), nil
		}
		time.Sleep(500 * time.Millisecond)
	}
	return time.Since(start), fmt.Errorf("not within %v", timeout)
}
