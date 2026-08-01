(() => {
  "use strict";

  const MAX_CAPTURED_CONSOLE_PARTS = 64;
  const MAX_PENDING_TIMERS = 64;
  const MAX_TIMER_DELAY_MS = 30 * 1000;
  const globalObject = globalThis;
  let invokeOperation;
  const queueUserTimer = Deno.core.queueUserTimer;
  const cancelTimer = Deno.core.cancelTimer;
  const stdout = [];
  const stderr = [];
  const pendingTimerIds = new Set();
  let maxCapturedConsoleBytes = 256 * 1024;
  let retainedConsoleBytes = 0;
  let captureTruncated = false;

  function format(arguments_) {
    let output = "";
    for (let index = 0; index < arguments_.length; index += 1) {
      if (index > 0) output += " ";
      const value = arguments_[index];
      if (typeof value === "string") output += value;
      else if (value === undefined) output += "undefined";
      else {
        try {
          const encoded = JSON.stringify(value);
          if (encoded !== undefined) output += encoded;
        } catch {
          output += String(value);
        }
      }
    }
    return output;
  }

  function capture(target, arguments_) {
    if (retainedConsoleBytes >= maxCapturedConsoleBytes) {
      captureTruncated = true;
      return;
    }
    const formatted = format(arguments_);
    const newlineBytes = target.length > 0 ? 1 : 0;
    let remaining = maxCapturedConsoleBytes - retainedConsoleBytes - newlineBytes;
    if (remaining < 0) remaining = 0;
    let bytes = 0;
    let end = 0;
    while (end < formatted.length) {
      const first = formatted.charCodeAt(end);
      let width = 1;
      let needed;
      if (first <= 0x7f) needed = 1;
      else if (first <= 0x7ff) needed = 2;
      else if (first >= 0xd800 && first <= 0xdbff && end + 1 < formatted.length) {
        const second = formatted.charCodeAt(end + 1);
        if (second >= 0xdc00 && second <= 0xdfff) {
          needed = 4;
          width = 2;
        } else needed = 3;
      } else needed = 3;
      if (bytes + needed > remaining) break;
      bytes += needed;
      end += width;
    }
    target.push(formatted.slice(0, end));
    retainedConsoleBytes += newlineBytes + bytes;
    if (end !== formatted.length) {
      retainedConsoleBytes = maxCapturedConsoleBytes;
      captureTruncated = true;
    }
    if (target.length >= MAX_CAPTURED_CONSOLE_PARTS) {
      target.splice(0, target.length, target.join("\n"));
    }
  }

  const consoleObject = Object.create(null);
  for (const name of ["log", "info", "debug"]) {
    Object.defineProperty(consoleObject, name, {
      value: (...arguments_) => capture(stdout, arguments_),
      configurable: false,
      writable: false,
    });
  }
  for (const name of ["error", "warn"]) {
    Object.defineProperty(consoleObject, name, {
      value: (...arguments_) => capture(stderr, arguments_),
      configurable: false,
      writable: false,
    });
  }
  Object.freeze(consoleObject);
  Object.defineProperty(globalObject, "console", {
    value: consoleObject,
    configurable: false,
    enumerable: true,
    writable: false,
  });

  Object.defineProperty(globalObject, "__codeModeInstallNamespace", {
    value: (descriptors, consoleLimit) => {
      maxCapturedConsoleBytes = consoleLimit;
      const namespace = Object.create(null);
      for (const [name, operation] of Object.entries(descriptors)) {
        Object.defineProperty(namespace, name, {
          value: async (input = {}) => await invokeOperation(operation, input),
          configurable: false,
          enumerable: true,
          writable: false,
        });
      }
      Object.freeze(namespace);
      Object.defineProperty(globalObject, "S2", {
        value: namespace,
        configurable: false,
        enumerable: true,
        writable: false,
      });
    },
    configurable: true,
    enumerable: false,
    writable: false,
  });

  Object.defineProperty(globalObject, "setTimeout", {
    value: (callback, delay = 0, ...arguments_) => {
      if (typeof callback !== "function") throw new TypeError("setTimeout requires a callback");
      if (pendingTimerIds.size >= MAX_PENDING_TIMERS) {
        throw new RangeError(`setTimeout permits at most ${MAX_PENDING_TIMERS} pending timers`);
      }
      const numericDelay = Number(delay);
      const boundedDelay = Number.isFinite(numericDelay)
        ? Math.min(Math.max(0, numericDelay), MAX_TIMER_DELAY_MS)
        : 0;
      let timerId;
      timerId = queueUserTimer(0, false, boundedDelay, () => {
        pendingTimerIds.delete(timerId);
        Reflect.apply(callback, undefined, arguments_);
      });
      pendingTimerIds.add(timerId);
      return timerId;
    },
    configurable: false,
    enumerable: true,
    writable: false,
  });
  Object.defineProperty(globalObject, "clearTimeout", {
    value: (timerId) => {
      if (pendingTimerIds.delete(timerId)) cancelTimer(timerId);
    },
    configurable: false,
    enumerable: true,
    writable: false,
  });

  Object.defineProperty(globalObject, "__codeModeRuntimeCapture", {
    value: () => ({ stdout: stdout.slice(), stderr: stderr.slice(), truncated: captureTruncated }),
    configurable: false,
    enumerable: false,
    writable: false,
  });

  Object.defineProperty(globalObject, "__codeModeFinalize", {
    value: () => {
      const operation = globalObject.Deno?.core?.ops?.op_codemode_invoke;
      if (typeof operation !== "function") {
        throw new Error("Code Mode invoke operation was not registered");
      }
      invokeOperation = operation;
      for (const name of [
        "Deno", "__bootstrap", "__infra", "process", "fetch", "WebSocket", "WebTransport",
        "EventSource", "XMLHttpRequest", "Request", "Response", "Headers", "FormData", "Worker",
        "SharedWorker", "BroadcastChannel", "MessageChannel", "MessagePort", "structuredClone",
        "crypto", "Blob", "File", "CompressionStream", "DecompressionStream", "ArrayBuffer",
        "SharedArrayBuffer", "DataView", "Int8Array", "Uint8Array", "Uint8ClampedArray",
        "Int16Array", "Uint16Array", "Int32Array", "Uint32Array", "BigInt64Array",
        "BigUint64Array", "Float16Array", "Float32Array", "Float64Array", "Atomics", "WebAssembly",
        "TextEncoder", "TextDecoder"
      ]) {
        if (!Reflect.deleteProperty(globalObject, name) || name in globalObject) {
          throw new Error(`failed to remove runtime global ${name}`);
        }
      }
    },
    configurable: true,
    enumerable: false,
    writable: false,
  });

})();
