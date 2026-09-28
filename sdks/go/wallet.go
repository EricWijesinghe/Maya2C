package maya2c

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"strings"
)

// Wallet is a keystore, the l1-wallet binary that can open it, and a node.
// The password reaches the child process in L1_WALLET_PASSWORD only; it
// never appears on a command line.
type Wallet struct {
	L1Wallet string
	Keystore string
	Client   *Client
	password string
}

// NewWallet returns a wallet signing through the Rust binary at l1Wallet.
func NewWallet(l1Wallet, keystore, password string, client *Client) *Wallet {
	return &Wallet{L1Wallet: l1Wallet, Keystore: keystore, Client: client, password: password}
}

func (w *Wallet) output(args ...string) (string, error) {
	full := append([]string{"--keystore", w.Keystore, "--rpc-url", w.Client.URL}, args...)
	cmd := exec.Command(w.L1Wallet, full...)
	cmd.Env = append(os.Environ(), "L1_WALLET_PASSWORD="+w.password)
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
	addr := strings.TrimSpace(out)
	if len(addr) != 64 {
		return "", fmt.Errorf("l1-wallet address printed %q", addr)
	}
	return addr, nil
}

// SignTransfer builds and signs a transfer without broadcasting it and
// returns the raw transaction and its txid.
func (w *Wallet) SignTransfer(to string, amount uint64) (raw, txid string, err error) {
	out, err := w.output("send", "--to", to, "--amount", fmt.Sprint(amount), "--no-broadcast")
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
func (w *Wallet) Transfer(to string, amount uint64) (string, error) {
	raw, _, err := w.SignTransfer(to, amount)
	if err != nil {
		return "", err
	}
	return w.Client.SendRawTransaction(raw)
}
