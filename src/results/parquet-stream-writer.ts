import type { Writer } from "hyparquet-writer";
import { ByteWriter } from "hyparquet-writer";

const DEFAULT_CHUNK_BYTES = 1_000_000;

export function parquetStreamWriter(
  writeChunk: (chunk: Uint8Array) => Promise<void>,
  chunkBytes: number = DEFAULT_CHUNK_BYTES
): Writer {
  const bytes = new ByteWriter();
  const growBuffer = bytes.ensure.bind(bytes);
  const pending: Uint8Array[] = [];

  const takeChunk = (): void => {
    if (bytes.index === 0) {
      return;
    }
    pending.push(new Uint8Array(bytes.buffer.slice(0, bytes.index)));
    bytes.index = 0;
  };

  const drain = async (): Promise<void> => {
    for (let chunk = pending.shift(); chunk; chunk = pending.shift()) {
      await writeChunk(chunk);
    }
  };

  return Object.assign(bytes, {
    ensure: (size: number): void => {
      if (bytes.index > chunkBytes) {
        takeChunk();
      }
      growBuffer(size);
    },
    flush: (): Promise<void> => {
      takeChunk();
      return drain();
    },
    finish: (): Promise<void> => {
      takeChunk();
      return drain();
    },
    getBuffer: (): ArrayBuffer => {
      throw new Error("parquetStreamWriter does not retain written bytes");
    },
    getBytes: (): Uint8Array<ArrayBuffer> => {
      throw new Error("parquetStreamWriter does not retain written bytes");
    },
  });
}
