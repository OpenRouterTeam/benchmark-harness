function requireEnv(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) {
    throw new Error(`Missing required environment variable ${name}`);
  }
  return value;
}

export function normalizeKeyPrefix(rawPrefix: string | undefined): string {
  const stripped = (rawPrefix ?? "").trim().replaceAll(/^\/+|\/+$/g, "");
  return stripped === "" ? "" : `${stripped}/`;
}

export function readMediaMirrorEnv() {
  return {
    endpoint: requireEnv("BENCH_MEDIA_S3_ENDPOINT"),
    bucket: requireEnv("BENCH_MEDIA_S3_BUCKET"),
    accessKeyId: requireEnv("BENCH_MEDIA_S3_ACCESS_KEY_ID"),
    secretAccessKey: requireEnv("BENCH_MEDIA_S3_SECRET_ACCESS_KEY"),
    publicBaseUrl: requireEnv("BENCH_MEDIA_PUBLIC_BASE_URL").replace(
      /\/+$/,
      ""
    ),
    keyPrefix: normalizeKeyPrefix(process.env["BENCH_MEDIA_KEY_PREFIX"]),
  };
}
