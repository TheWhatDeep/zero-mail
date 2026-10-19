// T21 and the section 8 link rules: what a click on a link in a message does.

import { describe, expect, it } from "vitest";
import { bodyUrl, comparableHost, decideLink, hostInText } from "./links";

describe("links in messages", () => {
  it("asks first when the text shows another host (T21)", () => {
    expect(decideLink("https://evil.test/login", "paypal.com")).toEqual({
      kind: "confirm",
      href: "https://evil.test/login",
      shownHost: "paypal.com",
      realHost: "evil.test",
    });
    expect(decideLink("https://paypal.com.evil.test/", "https://www.paypal.com/signin").kind).toBe("confirm");
    expect(decideLink("http://evil.test/", "PayPal.com/account").kind).toBe("confirm");
  });

  it("opens directly when the hosts match or the text is not a URL", () => {
    expect(decideLink("https://www.paypal.com/x", "paypal.com")).toEqual({ kind: "open", href: "https://www.paypal.com/x" });
    expect(decideLink("https://example.test/a", "Read more").kind).toBe("open");
    expect(decideLink("https://example.test/a", "example.test").kind).toBe("open");
    expect(decideLink("tel:+15551234", "Call us")).toEqual({ kind: "open", href: "tel:+15551234" });
  });

  it("allows only http, https, mailto and tel", () => {
    for (const href of ["javascript:alert(1)", "file:///c:/x", "data:text/html,x", "ftp://x.test/", "#", "not a url"]) {
      expect(decideLink(href, "x")).toEqual({ kind: "ignore" });
    }
    expect(decideLink("mailto:bob@example.test", "Bob")).toEqual({ kind: "compose", href: "mailto:bob@example.test" });
  });

  it("reads hosts from visible text", () => {
    expect(hostInText(" paypal.com ")).toBe("paypal.com");
    expect(hostInText("accounts.google.com/signin?x=1")).toBe("accounts.google.com");
    expect(hostInText("https://Bank.test:8443/p")).toBe("bank.test");
    expect(hostInText("Click here")).toBeNull();
    expect(hostInText("v1.2")).toBeNull();
    expect(comparableHost("WWW.Example.Test.")).toBe("example.test");
  });

  it("adds the plain-text toggle to the body URL", () => {
    expect(bodyUrl("mail-body://localhost/7?images=0", false)).toBe("mail-body://localhost/7?images=0");
    expect(bodyUrl("mail-body://localhost/7?images=1", true)).toBe("mail-body://localhost/7?images=1&plain=1");
  });
});
