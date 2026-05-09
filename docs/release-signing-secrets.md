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
| `APPLE_CERTIFICATE_PASSWORD` | `/Users/harper/workspace/icloud-2389/Apple/2389/Certificates.p12.password.txt` | Raw file contents. |
| `APPLE_SIGNING_IDENTITY` | literal value from `Certificates.p12` / `developerID_application.cer` | Use `Developer ID Application: 2389 Research, Inc (HD9NM9NSMK)`. |
| `APPLE_API_KEY_BASE64` | `/Users/harper/workspace/icloud-2389/Apple/2389/AuthKey_BWUFA73L84.p8` | Base64-encode the preferred App Store Connect admin API key and remove newlines before storing it in GitHub. |
| `APPLE_API_KEY_ID` | `/Users/harper/workspace/icloud-2389/Apple/2389/Key-id.txt` | Raw file contents. The value in this file must match the `.p8` selected for `APPLE_API_KEY_BASE64`. |
| `APPLE_API_ISSUER_ID` | `/Users/harper/workspace/icloud-2389/Apple/2389/Issuer-id.txt` | Raw file contents. |
| `APPLE_TEAM_ID` | `/Users/harper/workspace/icloud-2389/Apple/2389/Team-id.txt` | Raw file contents. |

`APPLE_CERTIFICATE_BASE64` and `APPLE_API_KEY_BASE64` are the only secrets that should be base64-encoded before upload.

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

The approved Task 1 mapping keeps `APPLE_API_KEY_ID` sourced from `Key-id.txt`, but the current local materials are mismatched:

- preferred API key file: `AuthKey_BWUFA73L84.p8`
- current `Key-id.txt` contents: `BHN2KMQ235`

Before running the `gh secret set` command for `APPLE_API_KEY_ID`, verify that `Key-id.txt` matches the `.p8` file you plan to upload for `APPLE_API_KEY_BASE64`. For the preferred admin key flow in this doc, `Key-id.txt` must contain `BWUFA73L84`. If it still contains `BHN2KMQ235`, stop and correct the local file or intentionally switch `APPLE_API_KEY_BASE64` to the matching `AuthKey_BHN2KMQ235.p8`.

## `gh secret set` Commands

Run these on a machine that both:

- has `gh` authenticated for `2389-research/barnstormer`
- has the Apple materials present at `/Users/harper/workspace/icloud-2389/Apple/2389`

```bash
base64 < /Users/harper/workspace/icloud-2389/Apple/2389/Certificates.p12 | tr -d '\n' | gh secret set APPLE_CERTIFICATE_BASE64 --repo 2389-research/barnstormer
gh secret set APPLE_CERTIFICATE_PASSWORD --repo 2389-research/barnstormer < /Users/harper/workspace/icloud-2389/Apple/2389/Certificates.p12.password.txt
printf '%s' 'Developer ID Application: 2389 Research, Inc (HD9NM9NSMK)' | gh secret set APPLE_SIGNING_IDENTITY --repo 2389-research/barnstormer
base64 < /Users/harper/workspace/icloud-2389/Apple/2389/AuthKey_BWUFA73L84.p8 | tr -d '\n' | gh secret set APPLE_API_KEY_BASE64 --repo 2389-research/barnstormer
gh secret set APPLE_API_KEY_ID --repo 2389-research/barnstormer < /Users/harper/workspace/icloud-2389/Apple/2389/Key-id.txt
gh secret set APPLE_API_ISSUER_ID --repo 2389-research/barnstormer < /Users/harper/workspace/icloud-2389/Apple/2389/Issuer-id.txt
gh secret set APPLE_TEAM_ID --repo 2389-research/barnstormer < /Users/harper/workspace/icloud-2389/Apple/2389/Team-id.txt
```

Task 1 only documents these commands. It does not run them.
