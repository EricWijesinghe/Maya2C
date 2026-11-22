package maya2c

import (
	"strings"
	"testing"
)

const testPassword = "hunter2-never-on-argv"

func TestWalletPasswordStaysOutOfArgv(t *testing.T) {
	w := NewWallet("/opt/l1-wallet", "/keys/main.json", testPassword, NewClient("http://127.0.0.1:8545"))
	cmd := w.command("send", "--to", "maya1x", "--amount", "5", "--no-broadcast")

	for _, arg := range cmd.Args {
		if strings.Contains(arg, testPassword) {
			t.Fatalf("password appears in argv: %q", cmd.Args)
		}
	}
	if !envHas(cmd.Env, "L1_WALLET_PASSWORD="+testPassword) {
		t.Fatal("password is not passed in L1_WALLET_PASSWORD")
	}
}

func envHas(env []string, want string) bool {
	for _, kv := range env {
		if kv == want {
			return true
		}
	}
	return false
}
