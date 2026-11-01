// SPDX-License-Identifier: AGPL-3.0-or-later
import { beforeEach, describe, expect, it } from "vitest";
import { MemoryBlobStore } from "./blobs.ts";
import { createJsHost, type JsWebSocketHandle } from "./host.ts";
import { MemKv } from "./idb.ts";
import { LocalLocks } from "./locks.ts";

type Listener = (event: unknown) => void;

/** A scripted browser WebSocket: tests choose exactly when each event reaches the host. */
class FakeWebSocket {
  static instances: FakeWebSocket[] = [];

  readonly sent: string[] = [];
  readonly listeners = new Map<string, Set<Listener>>();
  binaryType = "blob";
  closeCalls = 0;
  readyState = 0;

  constructor(
    readonly url: string,
    readonly protocols?: string | string[],
  ) {
    FakeWebSocket.instances.push(this);
  }

  addEventListener(type: string, listener: Listener): void {
    const listeners = this.listeners.get(type) ?? new Set<Listener>();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: Listener): void {
    this.listeners.get(type)?.delete(listener);
  }

  send(text: string): void {
    if (this.readyState !== 1) throw new Error("not open");
    this.sent.push(text);
  }

  close(): void {
    this.closeCalls++;
    this.readyState = 3;
  }

  open(): void {
    this.readyState = 1;
    this.emit("open");
  }

  message(data: string | ArrayBuffer): void {
    this.emit("message", data);
  }

  fail(): void {
    this.emit("error");
  }

  remoteClose(): void {
    this.readyState = 3;
    this.emit("close");
  }

  private emit(type: string, data?: unknown): void {
    for (const listener of [...(this.listeners.get(type) ?? [])]) listener({ type, data });
  }
}

function host() {
  return createJsHost({
    cache: new MemKv(),
    state: new MemKv(),
    fetch: {
      fetch: async () => {
        throw new Error("unused");
      },
      policy: { devnet: false },
    },
    userPds: async () => {
      throw new Error("unused");
    },
    blobs: new MemoryBlobStore(),
    locks: new LocalLocks(),
    webSocket: FakeWebSocket as unknown as typeof WebSocket,
  });
}

async function connected(maxBytes = 1024): Promise<[JsWebSocketHandle, FakeWebSocket]> {
  const opening = host().websocket("wss://jetstream.example/subscribe", maxBytes);
  const socket = FakeWebSocket.instances.at(-1) as FakeWebSocket;
  socket.open();
  return [await opening, socket];
}

beforeEach(() => {
  FakeWebSocket.instances = [];
});

describe("browser WebSocket host", () => {
  it("opens with the XRPC subprotocol, pulls text and binary FIFO, sends and closes", async () => {
    const url = "wss://jetstream.example/xrpc/network.bsky.jetstream.subscribeEvents?collections=x";
    const opening = host().websocket(url, 1024);
    const socket = FakeWebSocket.instances[0] as FakeWebSocket;
    expect(socket.url).toBe(url);
    expect(socket.protocols).toBe("xrpc.v1.json");
    expect(socket.binaryType).toBe("arraybuffer");
    socket.open();
    const ws = await opening;

    socket.message("first");
    socket.message(Uint8Array.from([0, 255]).buffer);
    socket.message("");
    await expect(ws.next()).resolves.toEqual({ kind: "message", text: "first" });
    await expect(ws.next()).resolves.toEqual({
      kind: "message",
      binary: Uint8Array.from([0, 255]),
    });
    await expect(ws.next()).resolves.toEqual({ kind: "message", text: "" });

    const update = JSON.stringify({
      type: "options_update",
      payload: { wantedDids: ["did:plc:a"] },
    });
    await ws.sendText(update);
    expect(socket.sent).toEqual([update]);
    await ws.close();
    await ws.close();
    expect(socket.closeCalls).toBe(1);
    await expect(ws.next()).resolves.toBeNull();
  });

  it("keeps independent FIFOs for separate sockets and drains frames queued before close", async () => {
    const h = host();
    const firstOpening = h.websocket("wss://one.example/subscribe", 1024);
    const secondOpening = h.websocket("wss://two.example/subscribe", 1024);
    const first = FakeWebSocket.instances[0] as FakeWebSocket;
    const second = FakeWebSocket.instances[1] as FakeWebSocket;
    first.open();
    second.open();
    const firstWs = await firstOpening;
    const secondWs = await secondOpening;

    first.message("a1");
    second.message("b1");
    first.message("a2");
    first.remoteClose();
    await expect(firstWs.next()).resolves.toEqual({ kind: "message", text: "a1" });
    await expect(secondWs.next()).resolves.toEqual({ kind: "message", text: "b1" });
    await expect(firstWs.next()).resolves.toEqual({ kind: "message", text: "a2" });
    await expect(firstWs.next()).resolves.toBeNull();
    await expect(firstWs.next()).resolves.toBeNull();
    second.message("b2");
    await expect(secondWs.next()).resolves.toEqual({ kind: "message", text: "b2" });
  });

  it("wakes a pending pull on close and reports an error after earlier queued frames", async () => {
    const [ws, socket] = await connected();
    const pending = ws.next();
    socket.remoteClose();
    await expect(pending).resolves.toBeNull();

    const [other, otherSocket] = await connected();
    otherSocket.message("before the error");
    otherSocket.fail();
    await expect(other.next()).resolves.toEqual({ kind: "message", text: "before the error" });
    await expect(other.next()).rejects.toMatchObject({ kind: "network" });
    await expect(other.next()).resolves.toBeNull();
    await expect(ws.sendText("closed")).rejects.toMatchObject({ kind: "network" });
  });

  it("rejects a connection that fails or closes before opening", async () => {
    const h = host();
    const failed = h.websocket("wss://jetstream.example/subscribe", 1024);
    (FakeWebSocket.instances[0] as FakeWebSocket).fail();
    await expect(failed).rejects.toMatchObject({ kind: "network" });

    const closed = h.websocket("wss://jetstream.example/subscribe", 1024);
    (FakeWebSocket.instances[1] as FakeWebSocket).remoteClose();
    await expect(closed).rejects.toMatchObject({ kind: "network" });
    await expect(h.websocket("https://jetstream.example/subscribe", 1024)).rejects.toMatchObject({
      kind: "blocked",
    });
  });

  it("measures text in UTF-8 bytes, closes on oversize, and preserves earlier frames", async () => {
    const [ws, socket] = await connected(3);
    socket.message("ok");
    // Two JavaScript characters, but four UTF-8 bytes.
    socket.message("éé");
    await expect(ws.next()).resolves.toEqual({ kind: "message", text: "ok" });
    await expect(ws.next()).rejects.toMatchObject({ kind: "tooLarge" });
    await expect(ws.next()).resolves.toBeNull();
    expect(socket.closeCalls).toBe(1);

    const [exact, exactSocket] = await connected(4);
    exactSocket.message("éé");
    await expect(exact.next()).resolves.toEqual({ kind: "message", text: "éé" });

    const [binary, binarySocket] = await connected(3);
    binarySocket.message(Uint8Array.from([1, 2, 3, 4]).buffer);
    await expect(binary.next()).rejects.toMatchObject({ kind: "tooLarge" });
  });
});
