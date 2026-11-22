package maya2c

import (
	"bytes"
	"fmt"
	"math/big"
	"os"
	"os/exec"
	"strings"
)

// Wallet is a keystore, the l1-wallet binary that can open it, and a node.
// The password reaches the child process in L1_WALLET_PASSWORD only; it
// never appears on a command line.
//
// Genesis is the network's published genesis id. l1-wallet signs without it
// only for a network its build pins (mainnet, the public testnet); for any
// other it must be set, and must match the node. Calling SignTransfer is the
// confirmation, so it passes --yes.
type Wallet struct {
	L1Wallet string
	Keystore string
	Client   *Client
	Genesis  string
	password string
}

// NewWallet returns a wallet signing through the Rust binary at l1Wallet.
func NewWallet(l1Wallet, keystore, password string, client *Client) *Wallet {
	return &Wallet{L1Wallet: l1Wallet, Keystore: keystore, Client: client, password: password}
}

// command builds the l1-wallet invocation. The password goes in the child's
// environment only: argv is visible to every user on the machine through the
// process list.
func (w *Wallet) command(args ...string) *exec.Cmd {
	full := append([]string{"--keystore", w.Keystore, "--rpc-url", w.Client.URL}, args...)
	cmd := exec.Command(w.L1Wallet, full...)
	cmd.Env = append(os.Environ(), "L1_WALLET_PASSWORD="+w.password)
	return cmd
}

func (w *Wallet) output(args ...string) (string, error) {
	cmd := w.command(args...)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("l1-wallet %s: %v: %s", args[0], err, strings.TrimSpace(stderr.String()))
	}
	return stdout.String(), nil
}

// Address returns the keystore's address.
func (w *Wallet) Address() (string, error) {
	out, err := w.output("address")
	if err != nil {
		return "", err
	}
	// The Bech32m text, then "hex: <64 hex>" (ADR-081); an older build
	// printed the hex alone.
	addr := strings.TrimSpace(out)
	for _, line := range strings.Split(addr, "\n") {
		if key, value, ok := strings.Cut(line, ":"); ok && strings.TrimSpace(key) == "hex" {
			addr = strings.TrimSpace(value)
		}
	}
	if len(addr) != 64 {
		return "", fmt.Errorf("l1-wallet address printed %q", strings.TrimSpace(out))
	}
	return addr, nil
}

// SignTransfer builds and signs a transfer without broadcasting it and
// returns the raw transaction and its txid.
func (w *Wallet) SignTransfer(to string, amount *big.Int) (raw, txid string, err error) {
	args := []string{"send", "--yes"}
	if w.Genesis != "" {
		args = append(args, "--genesis", w.Genesis)
	}
	args = append(args, "--to", to, "--amount", amount.String(), "--no-broadcast")
	out, err := w.output(args...)
	if err != nil {
		return "", "", err
	}
	for _, line := range strings.Split(out, "\n") {
		key, value, ok := strings.Cut(line, ":")
		if !ok {
			continue
		}
		switch strings.TrimSpace(key) {
		case "raw":
			raw = strings.TrimSpace(value)
		case "txid":
			txid = strings.TrimSpace(value)
		}
	}
	if raw == "" {
		return "", "", fmt.Errorf("l1-wallet printed no raw transaction")
	}
	return raw, txid, nil
}

// Transfer signs a transfer and submits it through the node.
func (w *Wallet) Transfer(to string, amount *big.Int) (string, error) {
	raw, _, err := w.SignTransfer(to, amount)
	if err != nil {
		return "", err
	}
	return w.Client.SendRawTransaction(raw)
}
