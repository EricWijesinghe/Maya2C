// The faucet form on /guides/testnet/. Plain script, no build step: it posts
// an address to the faucet and shows the faucet's own message, which is
// written for people ("This network address has already requested funds
// today..."), rather than inventing a second wording here.
(() => {
  const FAUCET = "https://faucet.maya2c.dev";
  // 64 hex characters, or the testnet's Bech32m text form (ADR-081): "tmaya1"
  // then 59 symbols. The faucet checks the checksum; this only catches typos early.
  const HEX = /^(0x)?[0-9a-fA-F]{64}$/;
  const TEXT = /^tmaya1[qpzry9x8gf2tvdw0s3jn54khce6mua7l]{59}$/i;

  const form = document.getElementById("faucet-form");
  if (!form) return;
  const input = document.getElementById("faucet-address");
  const button = form.querySelector("button");
  const output = document.getElementById("faucet-result");

  const show = (text, ok) => {
    output.textContent = text;
    output.dataset.state = ok ? "ok" : "error";
  };

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const typed = input.value.trim();
    const isText = TEXT.test(typed);
    if (!isText && !HEX.test(typed)) {
      show("That is not a Maya2C testnet address: it should be 64 hex characters or start with tmaya1.", false);
      return;
    }
    const address = isText ? typed.toLowerCase() : typed.replace(/^0x/, "");
    button.disabled = true;
    show("Asking the faucet…", true);
    try {
      const response = await fetch(`${FAUCET}/request`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ address }),
      });
      const body = await response.json().catch(() => ({}));
      if (response.ok) {
        show(`Sent ${body.amount} test coins. Transaction ${body.txid}. ` +
             "It lands in about a second.", true);
      } else {
        show(body.message || `The faucet refused the request (HTTP ${response.status}).`, false);
      }
    } catch {
      show("Could not reach the faucet. The testnet may be offline right now; " +
           "try again later.", false);
    } finally {
      button.disabled = false;
    }
  });
})();
