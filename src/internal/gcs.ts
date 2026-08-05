import { RETRYABLE_ERR_FN_DEFAULT, Storage } from "@google-cloud/storage";

import { Either } from "./either";
import { unknownErrorToString } from "./errors";
import { createLoggedRetryableErrorFn } from "./gcs-retry-observer";
import { eLog, iLog } from "./log";

export interface GcsError {
  readonly message: string;
}

export interface GcsDownloadInput {
  readonly bucketName: string;
  readonly objectPath: string;
}

export interface GcsDownloadResult {
  readonly content: Buffer;
}

export interface GcsUploadInput {
  readonly bucketName: string;
  readonly objectPath: string;
  readonly content: string | Buffer;
  readonly contentType?: string;
}

export interface GcsUploadResult {
  readonly gcsPath: string;
}

let storageSingleton: Storage | undefined;

function gcsClient(): Storage {
  storageSingleton ??= new Storage({
    retryOptions: {
      retryableErrorFn: createLoggedRetryableErrorFn(RETRYABLE_ERR_FN_DEFAULT),
    },
  });
  return storageSingleton;
}

export async function downloadFromGcs(
  input: GcsDownloadInput
): Promise<Either.Either<GcsDownloadResult, GcsError>> {
  const { bucketName, objectPath } = input;
  const file = gcsClient().bucket(bucketName).file(objectPath);
  try {
    const [content] = await file.download();
    iLog("Download from GCS succeeded", {
      bucket: bucketName,
      object_path: objectPath,
      file_size_bytes: content.length,
    });
    return Either.right({ content });
  } catch (error) {
    const message = unknownErrorToString(error);
    eLog("Failed to download from GCS", {
      bucket: bucketName,
      object_path: objectPath,
      error: message,
    });
    return Either.left({ message });
  }
}

export async function uploadToGcs(
  input: GcsUploadInput
): Promise<Either.Either<GcsUploadResult, GcsError>> {
  const {
    bucketName,
    objectPath,
    content,
    contentType = "application/octet-stream",
  } = input;
  const buffer =
    typeof content === "string" ? Buffer.from(content, "utf8") : content;
  const file = gcsClient().bucket(bucketName).file(objectPath);
  try {
    await file.save(buffer, { contentType });
    iLog("Upload to GCS succeeded", {
      bucket: bucketName,
      object_path: objectPath,
      file_size_bytes: buffer.length,
    });
    return Either.right({ gcsPath: `gs://${bucketName}/${objectPath}` });
  } catch (error) {
    const message = unknownErrorToString(error);
    eLog("Failed to upload to GCS", {
      bucket: bucketName,
      object_path: objectPath,
      error: message,
    });
    return Either.left({ message });
  }
}
