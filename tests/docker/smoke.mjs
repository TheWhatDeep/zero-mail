// Smoke check for the Stalwart test container: IMAP greets with STARTTLS on, SMTP banners.
// Usage (after `docker compose -f tests/docker/compose.yml up -d --wait`): node tests/docker/smoke.mjs
import { connect } from "node:net";

function greeting(port, expect) {
  return new Promise((resolve, reject) => {
    const socket = connect({ host: "127.0.0.1", port });
    let data = "";
    const timer = setTimeout(() => {
      socket.destroy();
      reject(new Error(`port ${port}: no greeting within 10 s`));
    }, 10_000);
    socket.on("data", (chunk) => {
      data += chunk.toString("latin1");
      if (data.includes("\n")) {
        clearTimeout(timer);
        socket.destroy();
        if (expect.test(data)) resolve(data.trim());
        else reject(new Error(`port ${port}: unexpected greeting ${JSON.stringify(data.trim())}`));
      }
    });
    socket.on("error", (err) => {
      clearTimeout(timer);
      reject(err);
    });
  });
}

console.log(await greeting(3143, /^\* OK .*STARTTLS/i));
console.log(await greeting(3025, /^220 /));
console.log(await greeting(3587, /^220 /));
console.log("stalwart smoke: ok");
