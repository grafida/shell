# Getting signing certificates

[Packaging](packaging.md) signs your macOS and Windows packages if the packager finds credentials in the environment. This
page is about getting those credentials in the first place: a **Developer ID** certificate and a **notarisation**
credential from Apple, and a **code-signing certificate** for Windows from Azure Artifact Signing. Both take a one-time
setup and a paid account. You can skip all of it for development: see [Unsigned builds](packaging.md#unsigned-builds).

Linux packages are not signed, so this page does not apply to them.

The steps below follow the vendors' portals and command-line tools as they were when this was written. Prices, role
names and menu labels change; if something differs, the vendor's current documentation wins.

## macOS: Developer ID and notarisation

For an app to open on other people's Macs without Gatekeeper blocking it, the bundle must be **signed with a Developer ID
Application certificate**, and the disk image **notarised by Apple and stapled**. That needs a paid
[Apple Developer Program](https://developer.apple.com/programs/) membership. You do the following once, on the Mac that
builds your packages.

### 1. Install the Command Line Tools

You do not need the full Xcode. The standalone Command Line Tools provide `codesign`, `notarytool` and `stapler`.

```sh
xcode-select --install
```

Confirm:

```sh
xcode-select -p            # /Library/Developer/CommandLineTools
xcrun notarytool --version
codesign --version
```

If `xcode-select -p` points at an Xcode you do not want to use, switch with
`sudo xcode-select -s /Library/Developer/CommandLineTools`.

### 2. Create a "Developer ID Application" certificate

This is the identity that signs apps for direct download. It must be **Developer ID Application**: "Apple Development" and
"Mac App Store" certificates do not work for a notarised download. Without Xcode, you create it in the Developer portal,
with a certificate signing request (CSR) made by Keychain Access.

1. **Generate a CSR.** Open **Keychain Access**, then **Keychain Access ▸ Certificate Assistant ▸ Request a Certificate
   From a Certificate Authority…**. Enter your email address, put your name in *Common Name*, leave *CA Email* empty,
   choose **Saved to disk**, and save the `.certSigningRequest` file.
2. **Create the certificate.** Open the [Certificates page](https://developer.apple.com/account/resources/certificates)
   of your developer account, click **+**, choose **Developer ID Application** (under *Software*), upload the CSR and
   download the resulting `.cer` file.
3. **Install it.** Double-click the `.cer` file to import it into your **login keychain**. The private key was created
   with the CSR, on this Mac, so the certificate becomes a usable signing identity. Make the CSR on the machine that
   will sign; the certificate is useless without that key.

> ⚠️ **Do not change the certificate's trust settings.** Leave "When using this certificate" on **Use System
> Defaults**. Setting your Developer ID certificate to "Always Trust" turns it into a trust anchor, and `codesign` then
> fails to build a chain with `unable to build chain to self-signed root … errSecInternalComponent`. If you already did,
> open the certificate in Keychain Access, expand **Trust** and put it back to **Use System Defaults**.

Check that it is there:

```sh
security find-identity -v -p codesigning
```

The output has a line such as:

```
1) A1B2C3…  "Developer ID Application: Example Ltd (ABCDE12345)"
```

The quoted text is your **signing identity**, which goes into `MACOS_SIGN_IDENTITY`. The ten characters in parentheses are
your **Team ID**.

### 3. Install Apple's intermediate and root certificates

A machine with only the Command Line Tools needs this step; a full Xcode does it for you. Your certificate is the end of a
chain (`Developer ID Application → Developer ID Certification Authority → Apple Root CA`). If the two upstream
certificates are missing from the keychain, `codesign` fails with `unable to build chain to self-signed root for signer …
errSecInternalComponent`.

Download them from Apple's public certificate authority page and import them into the login keychain:

```sh
cd "$(mktemp -d)"
curl -fsSL -O https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer
curl -fsSL -o AppleRootCA.cer https://www.apple.com/appleca/AppleIncRootCertificate.cer
security import DeveloperIDG2CA.cer -k ~/Library/Keychains/login.keychain-db
security import AppleRootCA.cer     -k ~/Library/Keychains/login.keychain-db
```

* "Already exists" from `security import` is fine.
* Do not change any trust setting on these either.
* If your certificate was issued by a different intermediate (`openssl x509 -in cert.pem -noout -issuer` shows the issuer),
  download that one from [apple.com/certificateauthority](https://www.apple.com/certificateauthority/).

### 4. Store a notarisation credential

Notarisation uploads the disk image to Apple, which scans it and returns a ticket. `notarytool` needs to authenticate; the
simplest way is an **app-specific password**.

1. Sign in at [appleid.apple.com](https://appleid.apple.com), go to **Sign-In and Security ▸ App-Specific Passwords**,
   create one and copy it (`abcd-efgh-ijkl-mnop`).
2. Store it once as a keychain profile:

   ```sh
   xcrun notarytool store-credentials "myapp-notary" \
     --apple-id "you@example.com" \
     --team-id "ABCDE12345" \
     --password "abcd-efgh-ijkl-mnop"
   ```

   `myapp-notary` is the **profile name**; it goes into `MACOS_NOTARY_PROFILE`. Check it works:

   ```sh
   xcrun notarytool history --keychain-profile "myapp-notary"
   ```

An alternative that suits CI is an App Store Connect API key:
`xcrun notarytool store-credentials NAME --key AuthKey_XXXX.p8 --key-id XXXX --issuer <uuid>`. The result is a profile
in the same way.

### 5. Tell the packager

Export the two names before building. They are only names; the certificate and the notarisation credential stay in the
keychain.

```sh
export MACOS_SIGN_IDENTITY="Developer ID Application: Example Ltd (ABCDE12345)"
export MACOS_NOTARY_PROFILE="myapp-notary"
```

Then build as in [Packaging: macOS](packaging.md#macos). If signing fails, `codesign --verify --deep --strict --verbose=2
MyApp.app` and `spctl --assess --type execute --verbose MyApp.app` show what Gatekeeper sees. A notarised, stapled disk
image passes `xcrun stapler validate MyApp.dmg`.

## Windows: Azure Artifact Signing

Azure Artifact Signing (formerly Azure Trusted Signing) is Microsoft's cloud signing service. The private key lives in
Azure's hardware security modules, so there is no USB token to plug in, and no certificate file to protect. The packager
signs with [Jsign](https://ebourg.github.io/jsign/), which runs on macOS, Linux and Windows, so you can sign Windows
packages from any of them.

If you prefer a conventional certificate from a certificate authority, skip this section: export it as a PKCS#12 file and
use `WINDOWS_SIGN_PKCS12` (see [Providing signing credentials](packaging.md#providing-signing-credentials)).

Things to know first:

* **Cost.** The Basic tier was about €10 a month at the time of writing, with 5,000 signatures. Check Azure's pricing
  page.
* **Subscription.** It does not work on free, trial or sponsored subscriptions. You need a pay-as-you-go or enterprise
  subscription.
* **Identity validation takes time.** Microsoft must verify who you are before it issues publicly trusted certificates. It
  is the slow step, from a few days to two weeks. Start it first.
* **Certificates are short-lived** (three days, by design). That is harmless **if every signature carries a trusted
  timestamp**: the signature stays valid after the certificate expires. The packager always timestamps. Never sign
  without a timestamp by hand.
* **It does not remove SmartScreen warnings by itself.** SmartScreen reputation builds with download volume over time. A
  signed file shows your publisher's name instead of "Unknown publisher", and it behaves better with antivirus and
  AppLocker policies, but a new app with few downloads can still show "Windows protected your PC". See
  [Unsigned builds: Windows](packaging.md#windows-1).

### What you need on your machine

The [Azure CLI](https://learn.microsoft.com/cli/azure/install-azure-cli) (`az`), to set up the account and to get access
tokens, and Jsign. On macOS with Homebrew:

```sh
brew install azure-cli jsign osslsigncode
```

(`osslsigncode` is used by the packager to check the finished signature; building the installer also needs `makensis`, as
described in [Packaging](packaging.md#install-the-packager).) The steps below use shell variables such as `$RESOURCE_ID`;
on Windows use PowerShell equivalents or the Azure portal.

### 1. A paid subscription

Sign in at [portal.azure.com](https://portal.azure.com) and make sure you have a pay-as-you-go or enterprise
subscription. Then log in from the command line:

```sh
az login
```

### 2. Register the resource provider

```sh
az provider register --namespace Microsoft.CodeSigning

# Wait for "Registered" (two or three minutes)
az provider show --namespace Microsoft.CodeSigning --query registrationState -o tsv
```

### 3. Create a resource group and the signing account

Pick a region close to you; the account name is yours to choose and appears in the signing profile later.

```sh
az group create --name MyAppCodeSigning --location westeurope

az trustedsigning create \
  --name myapp-signing \
  --resource-group MyAppCodeSigning \
  --location westeurope \
  --sku Basic
```

If `az trustedsigning` is unknown, install the extension with `az extension add --name trustedsigning`, or create the
account in the portal instead (search for "Trusted Signing"; the portal lists it as **Artifact Signing Accounts**).

### 4. Give yourself the Identity Verifier role

To submit an identity validation, your own Azure user needs the *Artifact Signing Identity Verifier* role on the account:

```sh
RESOURCE_ID=$(az trustedsigning show --name myapp-signing --resource-group MyAppCodeSigning --query id -o tsv)
MY_OBJECT_ID=$(az ad signed-in-user show --query id -o tsv)

az role assignment create \
  --assignee "$MY_OBJECT_ID" \
  --role "Artifact Signing Identity Verifier" \
  --scope "$RESOURCE_ID"
```

Wait a minute or two for the role to take effect.

### 5. Submit the identity validation

This is the one manual step, and nothing after it works until Microsoft approves it. In the Azure portal:

1. Search for **Trusted Signing** and open **Artifact Signing Accounts**, then your account.
2. In the left menu open **Identity validation** and choose **New identity validation**.
3. Choose **Individual** or **Organization**, as applies to you, and fill in the details the portal asks for. They must
   match your official documents; an organisation needs to be verifiable by Microsoft.
4. Submit, and wait for the approval email.

The name that is validated is the **publisher name** your users see on the signed installer.

### 6. Create a certificate profile

When the validation is approved, return to the account and open **Certificate profiles ▸ New**.

* **Profile type: Public Trust.** This matters. "Private Trust" profiles are for internal enterprise apps and do not
  produce signatures the public's Windows trusts.
* Give the profile a name, for example `myapp-public`. Remember it.

The packager's `WINDOWS_SIGN_PROFILE` is `<account>/<profile>`, here `myapp-signing/myapp-public`.

### 7. Create a service principal for non-interactive signing

A build must sign without a person logging in, so create an identity just for signing:

```sh
az ad sp create-for-rbac --name "myapp-codesigning" --output json
```

The output contains three values:

```json
{
  "appId": "<client-id>",
  "password": "<client-secret>",
  "tenant": "<tenant-id>"
}
```

**Store all three immediately, in a password manager or your CI's secret store.** The `password` is shown only once and
cannot be retrieved afterwards. It is a credential that can sign as you: guard it as you would a private key.

### 8. Let the service principal sign

```sh
SP_OBJECT_ID=$(az ad sp show --id <appId> --query id -o tsv)

az role assignment create \
  --assignee "$SP_OBJECT_ID" \
  --role "Artifact Signing Certificate Profile Signer" \
  --scope "$RESOURCE_ID"
```

### 9. Tell the packager

The packager needs three variables (see [Providing signing credentials](packaging.md#providing-signing-credentials)):

| Variable | Value |
|---|---|
| `WINDOWS_SIGN_ENDPOINT` | The **Account URI** from the account's overview page, for example `weu.codesigning.azure.net` for West Europe. It is specific to the region. Give the host name only, without `https://`; it is passed to Jsign as its keystore. |
| `WINDOWS_SIGN_PROFILE` | `<account>/<profile>`, for example `myapp-signing/myapp-public` |
| `WINDOWS_SIGN_TOKEN` | A short-lived access token, which you get just before building |

Log in as the service principal, then request the token:

```sh
az login --service-principal -u "<appId>" -p "<password>" --tenant "<tenant-id>"

export WINDOWS_SIGN_ENDPOINT="weu.codesigning.azure.net"
export WINDOWS_SIGN_PROFILE="myapp-signing/myapp-public"
export WINDOWS_SIGN_TOKEN="$(az account get-access-token --resource https://codesigning.azure.net --query accessToken -o tsv)"
```

(The `-p` argument is visible to other users of the machine while the command runs; on a shared machine, prefer your CI's
secret mechanism.) Tokens expire, usually within an hour, so fetch it in the same step as the build.

### Signing a file by hand

To check the Azure side without a full build, sign one file:

```sh
jsign \
  --storetype TRUSTEDSIGNING \
  --keystore "$WINDOWS_SIGN_ENDPOINT" \
  --storepass "$WINDOWS_SIGN_TOKEN" \
  --alias "$WINDOWS_SIGN_PROFILE" \
  --tsaurl http://timestamp.acs.microsoft.com \
  --tsmode RFC3161 \
  myapp.exe
```

`--tsaurl` is not optional (see "Certificates are short-lived", above). Then `osslsigncode verify myapp.exe`. On a Mac
or Linux machine it will report the signature as **untrusted**, even when it is right, because Microsoft's root
certificate is not in that system's trust store. Read the rest of its output (signer, timestamp, certificate chain) to see
that the signature is there; do not rely on its exit status. The packager's own check compares the signed digest with the
file, and verifies the chain only if `WINDOWS_SIGN_CAFILE` names the roots to check against.

### When signing fails with `403 Forbidden`

Jsign reports Azure's refusal as a bare `HTTP Error 403 - Forbidden`. Check, in this order:

1. **The names.** `WINDOWS_SIGN_PROFILE` must be exactly `<account>/<profile>` as the portal shows them, and
   `WINDOWS_SIGN_ENDPOINT` the Account URI of that account's region. A leftover from an earlier setup, with a different
   account or profile name, is the commonest cause.
2. **New terms of service.** Microsoft updates the Artifact Signing terms from time to time, and signing is refused until
   you accept them again. In the portal open your account, **Objects ▸ Identity validations**; if it asks you to accept
   updated terms, accept them. Nothing else tells you.
3. **The subscription is active.** An expired payment card disables it, and its resources then answer 403.
4. **The certificate profile is Active**, and the service principal still holds *Artifact Signing Certificate Profile
   Signer* on the account (**Access control (IAM) ▸ Role assignments**).

To see Azure's real reason, repeat the request Jsign makes. It prints the full response:

```sh
curl -sS -i -X POST \
  "https://$WINDOWS_SIGN_ENDPOINT/codesigningaccounts/<account>/certificateprofiles/<profile>/sign?api-version=2022-06-15-preview" \
  -H "Authorization: Bearer $WINDOWS_SIGN_TOKEN" -H "Content-Type: application/json" \
  -d "{\"signatureAlgorithm\":\"RS256\",\"digest\":\"$(head -c 32 /dev/zero | base64)\"}"
```

A 401 instead of a 403 means the token itself is wrong or expired, not the permissions.

## Keeping the credentials safe

* The Apple certificate's **private key** exists only in the keychain of the Mac where you made the CSR. Back it up
  (export a `.p12` from Keychain Access and keep it somewhere safe), or you must start again with a new certificate if
  that Mac is lost.
* The Azure **client secret** is shown once. Keep it in a password manager or your CI's secret store, never in
  `shell-package.json`, never in the repository.
* Never put either in the app's own repository history. A leaked signing credential lets someone else sign software as you.
