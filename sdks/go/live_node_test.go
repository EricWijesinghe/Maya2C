package maya2c

import (
	"errors"
	"os"
	"testing"
	"time"
)

// Against a live devnet: cargo xtask sdk-e2e --lang go sets it up.
func setup(t *testing.T) (*Client, *Wallet) {
	for _, k := range []string{"MAYA_RPC_URL", "L1_WALLET", "MAYA_KEYSTORE", "L1_WALLET_PASSWORD", "MAYA_RECIPIENT"} {
		if os.Getenv(k) == "" {
			t.Skip("needs a devnet: cargo xtask sdk-e2e --lang go")
		}
	}
	c := NewClient(os.Getenv("MAYA_RPC_URL"))
	return c, NewWallet(os.Getenv("L1_WALLET"), os.Getenv("MAYA_KEYSTORE"), os.Getenv("L1_WALLET_PASSWORD"), c)
}

func TestReadsTheChain(t *testing.T) {
	c, w := setup(t)
	sender, err := w.Address()
	if err != nil {
		t.Fatal(err)
	}
	tip, err := c.AccountAtTip(sender)
	if err != nil || tip.Balance == 0 || len(tip.BlockID) != 64 {
		t.Fatalf("account at tip: %+v, %v", tip, err)
	}
	b, err := c.BlockByHeight(1)
	if err != nil || b.Height != 1 || len(b.Header.ID) != 64 {
		t.Fatalf("block 1: %+v, %v", b, err)
	}
}

func TestATransferSignedInRustIsCredited(t *testing.T) {
	c, w := setup(t)
	to := os.Getenv("MAYA_RECIPIENT")
	before, err := c.Balance(to)
	if err != nil {
		t.Fatal(err)
	}
	txid, err := w.Transfer(to, 4321)
	if err != nil || len(txid) != 64 {
		t.Fatalf("transfer: %q, %v", txid, err)
	}
	waited, err := WaitUntil(func() bool {
		now, err := c.Balance(to)
		return err == nil && now == before+4321
	}, 60*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("go sdk: transfer %s... credited in %v", txid[:16], waited.Round(time.Millisecond))
}

func TestARefusalIsAnError(t *testing.T) {
	c, _ := setup(t)
	_, err := c.SendRawTransaction("00")
	var rpc *RPCError
	if !errors.As(err, &rpc) {
		t.Fatalf("a malformed transaction was not refused: %v", err)
	}
}
