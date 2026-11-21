import { z } from "zod";

export const MAX_BIO_LENGTH = 280;

const urlField = z
  .string()
  .optional()
  .refine(
    (v) => !v || v.startsWith("https://") || v.startsWith("http://"),
    "Must be a valid URL starting with http:// or https://"
  );

// ─── Bio-only (used inline in ProfileBio) ─────────────────────────────────────

export const profileBioSchema = z.object({
  bio: z
    .string()
    .max(MAX_BIO_LENGTH, `Bio must be ${MAX_BIO_LENGTH} characters or less`)
    .optional(),
  twitter: z.string().max(100).optional(),
  discord: z.string().max(100).optional(),
  twitch: z.string().max(100).optional(),
});

export type ProfileBioFormData = z.infer<typeof profileBioSchema>;

// ─── Full profile edit ────────────────────────────────────────────────────────

export const profileEditSchema = z.object({
  username: z
    .string()
    .min(3, "Username must be at least 3 characters")
    .max(20, "Username must be at most 20 characters")
    .regex(
      /^[a-zA-Z0-9]+$/,
      "Username can only contain letters and numbers"
    ),
  bio: z
    .string()
    .max(MAX_BIO_LENGTH, `Bio must be ${MAX_BIO_LENGTH} characters or less`)
    .optional(),
  twitter: urlField,
  discord: z.string().max(100, "Discord handle is too long").optional(),
  twitch: urlField,
  github: urlField,
});

export type ProfileEditFormData = z.infer<typeof profileEditSchema>;

// ─── Avatar upload ───────────────────────────────────────────────────────────

/** Maximum accepted avatar upload size in bytes (5 MiB). */
export const MAX_AVATAR_SIZE_BYTES = 5 * 1024 * 1024;

/** Maximum accepted avatar width/height in pixels. */
export const MAX_AVATAR_DIMENSION = 4096;

/** Minimum accepted avatar width/height in pixels. */
export const MIN_AVATAR_DIMENSION = 16;

/** MIME types accepted for avatar uploads. */
export const AVATAR_ALLOWED_MIME_TYPES = [
  "image/jpeg",
  "image/png",
  "image/webp",
] as const;

export type AvatarMimeType = (typeof AVATAR_ALLOWED_MIME_TYPES)[number];

export const AVATAR_ALLOWED_MIME_TYPES_LABEL = "JPEG, PNG, or WebP";

export const AVATAR_ERROR_MESSAGES = {
  invalidType: `Avatar must be a ${AVATAR_ALLOWED_MIME_TYPES_LABEL} image.`,
  emptyFile: "Avatar file is empty. Please choose a different image.",
  tooLarge: `Avatar must be ${MAX_AVATAR_SIZE_BYTES / (1024 * 1024)} MB or smaller.`,
  dimensionsTooLarge: `Avatar dimensions must be ${MAX_AVATAR_DIMENSION}x${MAX_AVATAR_DIMENSION} pixels or smaller.`,
  dimensionsTooSmall: `Avatar dimensions must be at least ${MIN_AVATAR_DIMENSION}x${MIN_AVATAR_DIMENSION} pixels.`,
  invalidDimensions: "Could not read avatar dimensions. Please choose a different image.",
} as const;

/**
 * Validates the raw file metadata for an avatar upload.
 * Does not perform decoding; callers should additionally verify dimensions
 * after decoding (see `avatarDimensionsSchema`).
 */
export const avatarFileSchema = z.object({
  type: z
    .string()
    .refine(
      (v): v is AvatarMimeType =>
        (AVATAR_ALLOWED_MIME_TYPES as readonly string[]).includes(v),
      AVATAR_ERROR_MESSAGES.invalidType
    ),
  size: z
    .number()
    .int("Avatar file size is invalid.")
    .positive(AVATAR_ERROR_MESSAGES.emptyFile)
    .max(MAX_AVATAR_SIZE_BYTES, AVATAR_ERROR_MESSAGES.tooLarge),
});

export type AvatarFileInput = z.infer<typeof avatarFileSchema>;

/**
 * Validates decoded avatar dimensions. Rejects non-finite, non-integer,
 * zero, negative, or out-of-bounds values.
 */
export const avatarDimensionsSchema = z.object({
  width: z
    .number()
    .int(AVATAR_ERROR_MESSAGES.invalidDimensions)
    .min(MIN_AVATAR_DIMENSION, AVATAR_ERROR_MESSAGES.dimensionsTooSmall)
    .max(MAX_AVATAR_DIMENSION, AVATAR_ERROR_MESSAGES.dimensionsTooLarge),
  height: z
    .number()
    .int(AVATAR_ERROR_MESSAGES.invalidDimensions)
    .min(MIN_AVATAR_DIMENSION, AVATAR_ERROR_MESSAGES.dimensionsTooSmall)
    .max(MAX_AVATAR_DIMENSION, AVATAR_ERROR_MESSAGES.dimensionsTooLarge),
});

export type AvatarDimensions = z.infer<typeof avatarDimensionsSchema>;

/**
 * Combined avatar upload schema: validates file metadata and, when provided,
 * decoded dimensions.
 */
export const avatarUploadSchema = avatarFileSchema.extend({
  width: avatarDimensionsSchema.shape.width.optional(),
  height: avatarDimensionsSchema.shape.height.optional(),
});

export type AvatarUploadInput = z.infer<typeof avatarUploadSchema>;

export type AvatarValidationResult =
  | { ok: true; data: AvatarUploadInput }
  | { ok: false; error: string };

/**
 * Convenience wrapper that returns a recoverable, user-visible error message
 * instead of throwing. Safe to call from client components.
 */
export function validateAvatarUpload(input: {
  type?: string | null;
  size?: number | null;
  width?: number | null;
  height?: number | null;
}): AvatarValidationResult {
  const parsed = avatarUploadSchema.safeParse({
    type: input.type ?? "",
    size: typeof input.size === "number" ? input.size : Number.NaN,
    width: typeof input.width === "number" ? input.width : undefined,
    height: typeof input.height === "number" ? input.height : undefined,
  });

  if (parsed.success) {
    return { ok: true, data: parsed.data };
  }

  const first = parsed.error.issues[0];
  return {
    ok: false,
    error: first?.message ?? "Avatar upload is invalid. Please choose a different image.",
  };
}