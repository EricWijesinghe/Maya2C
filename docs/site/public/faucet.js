// The faucet form on /guides/testnet/. Plain script, no build step: it posts
// an address to the faucet and shows the faucet's own message, which is
// written for people ("This network address has already requested funds
// today..."), rather than inventing a second wording here.
(() => {
  const FAUCET = "https://faucet.maya2c.dev";
  const ADDRESS = /^(0x)?[0-9a-fA-F]{64}$/;

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
    const address = input.value.trim().replace(/^0x/, "");
    if (!ADDRESS.test(address)) {
      show("That is not a Maya2C address: it should be 64 hex characters.", false);
      return;
    }
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
