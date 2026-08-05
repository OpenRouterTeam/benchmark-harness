import type { ApiError } from "@google-cloud/storage";

import { truncatedMessage } from "../runtime/retry";
import { wLog } from "./log";

export type RetryableErrorPredicate = (error: ApiError) => boolean;

export function createLoggedRetryableErrorFn(
  defaultPredicate: RetryableErrorPredicate
): RetryableErrorPredicate {
  return (error) => {
    const isRetryable = defaultPredicate(error);
    if (isRetryable) {
      wLog("GCS operation hit retryable error", {
        error_message: truncatedMessage(error),
        error_code: error?.code,
        error_status: error?.response?.statusCode,
      });
    }
    return isRetryable;
  };
}
