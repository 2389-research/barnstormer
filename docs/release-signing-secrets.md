# Release Signing Secrets

This repo's macOS desktop release flow uses repo-level GitHub secrets on `2389-research/barnstormer`. The local source of truth for the Apple materials is:

```text
/Users/harper/workspace/icloud-2389/Apple/2389
```

## Required Secrets Checklist

- `APPLE_CERTIFICATE_BASE64`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_API_KEY_BASE64`
- `APPLE_API_KEY_ID`
- `APPLE_API_ISSUER_ID`
- `APPLE_TEAM_ID`

## Secret Contract

| GitHub secret | Local source | Notes |
|---|---|---|
| `APPLE_CERTIFICATE_BASE64` | `/Users/harper/workspace/icloud-2389/Apple/2389/Certificates.p12` | Base64-encode the full `.p12` and remove newlines before storing it in GitHub. |
| `APPLE_CERTIFICATE_PASSWORD` | `/Users/harper/workspace/icloud-2389/Apple/2389/Certificates.p12.password.txt` | Raw text secret. Trim any trailing newline before storing it in GitHub. |
| `APPLE_SIGNING_IDENTITY` | literal value from `Certificates.p12` / `developerID_application.cer` | Use `Developer ID Application: 2389 Research, Inc (HD9NM9NSMK)`. |
| `APPLE_API_KEY_BASE64` | `/Users/harper/workspace/icloud-2389/Apple/2389/AuthKey_BHN2KMQ235.p8` | Base64-encode the matching App Store Connect API key and remove newlines before storing it in GitHub. |
| `APPLE_API_KEY_ID` | `/Users/harper/workspace/icloud-2389/Apple/2389/Key-id.txt` | Raw text secret. Trim any trailing newline before storing it in GitHub. The value in this file must match the `.p8` selected for `APPLE_API_KEY_BASE64`. |
| `APPLE_API_ISSUER_ID` | `/Users/harper/workspace/icloud-2389/Apple/2389/Issuer-id.txt` | Raw text secret. Trim any trailing newline before storing it in GitHub. |
| `APPLE_TEAM_ID` | `/Users/harper/workspace/icloud-2389/Apple/2389/Team-id.txt` | Raw text secret. Trim any trailing newline before storing it in GitHub. |

`APPLE_CERTIFICATE_BASE64` and `APPLE_API_KEY_BASE64` are the only secrets that should be base64-encoded before upload.

All text secrets must be stored without trailing newline characters.

## Materials Not Needed For This Flow

These local files are not part of the Tauri macOS release-signing path and should not be uploaded as GitHub secrets for this workflow:

- any provisioning profile, including `.mobileprovision` and `.provisionprofile` files
- `CertificateSigningRequest.certSigningRequest`
- `distribution_new.csr`
- `distribution_private.key`
- `distribution.cer`
- `DistCertificates.p12`
- `apple_dist.p12`

This release path is `Developer ID Application` signing plus notarization. It is not an App Store or iOS provisioning flow.

## Preflight Check For The API Key Pair

Before running the `gh secret set` commands, verify that `Key-id.txt` matches the `.p8` file you plan to upload for `APPLE_API_KEY_BASE64`. The current local pairing documented here is:

- API key file: `AuthKey_BHN2KMQ235.p8`
- `Key-id.txt` contents: `BHN2KMQ235`

## `gh secret set` Commands

Run these on a machine that both:

- has `gh` authenticated for `2389-research/barnstormer`
- has the Apple materials present at `/Users/harper/workspace/icloud-2389/Apple/2389`

```bash
base64 < /Users/harper/workspace/icloud-2389/Apple/2389/Certificates.p12 | tr -d '\n' | gh secret set APPLE_CERTIFICATE_BASE64 --repo 2389-research/barnstormer
tr -d '\n' < /Users/harper/workspace/icloud-2389/Apple/2389/Certificates.p12.password.txt | gh secret set APPLE_CERTIFICATE_PASSWORD --repo 2389-research/barnstormer
printf '%s' 'Developer ID Application: 2389 Research, Inc (HD9NM9NSMK)' | gh secret set APPLE_SIGNING_IDENTITY --repo 2389-research/barnstormer
base64 < /Users/harper/workspace/icloud-2389/Apple/2389/AuthKey_BHN2KMQ235.p8 | tr -d '\n' | gh secret set APPLE_API_KEY_BASE64 --repo 2389-research/barnstormer
tr -d '\n' < /Users/harper/workspace/icloud-2389/Apple/2389/Key-id.txt | gh secret set APPLE_API_KEY_ID --repo 2389-research/barnstormer
tr -d '\n' < /Users/harper/workspace/icloud-2389/Apple/2389/Issuer-id.txt | gh secret set APPLE_API_ISSUER_ID --repo 2389-research/barnstormer
tr -d '\n' < /Users/harper/workspace/icloud-2389/Apple/2389/Team-id.txt | gh secret set APPLE_TEAM_ID --repo 2389-research/barnstormer
```

Task 1 only documents these commands. It does not run them.
