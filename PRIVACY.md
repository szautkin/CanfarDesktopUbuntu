# Privacy Policy — Verbinal for Linux

**Effective date:** 1 March 2025
**App name:** Verbinal — A CANFAR Science Portal Companion (Linux)
**Publisher:** CodeBG (Serhii Zautkin)

---

## Summary

Verbinal does not collect, transmit, or sell any personal data to the developer,
to CodeBG, or to any third party. All data stays on your device or is sent
directly to the CANFAR services you choose to authenticate with.

---

## 1. Data we collect

**None.** Verbinal does not have its own backend, analytics, telemetry,
crash-reporting service, or advertising SDK. The app contains no tracking code
of any kind.

## 2. Data stored on your device

Verbinal stores the following information locally:

| Data | Location | Purpose |
|---|---|---|
| CANFAR authentication token | System keyring (Secret Service / GNOME Keyring / KDE Wallet) | Keeps you signed in between sessions when "Remember me" is checked |
| CANFAR username | System keyring | Identifies the account associated with the saved token |
| Recent session launches | `recent_launches.json` in XDG data directory | Shows your recent session history for quick re-launch |

All locally stored data is deleted when you log out (which clears credentials
from the system keyring).

## 3. Data sent over the network

Verbinal communicates exclusively with CANFAR services operated by the Canadian
Astronomy Data Centre (CADC) and the Digital Research Alliance of Canada. All
connections use HTTPS.

| Endpoint | Data sent | Purpose |
|---|---|---|
| `ws-cadc.canfar.net` | Username and password (at login) | Authentication |
| `ws-uv.canfar.net/skaha` | Authentication token (Bearer header) | Session management, image listing, platform stats |
| `ws-uv.canfar.net/ac` | Authentication token | User profile retrieval |
| `ws-uv.canfar.net/arc` | Authentication token | Storage quota retrieval |

Verbinal does **not** contact any other servers. There are no analytics
endpoints, no ad networks, and no third-party SDKs that make network requests.

Your credentials are sent only to the CANFAR authentication endpoint
(`ws-cadc.canfar.net/ac/login`) and are never stored in plain text on disk.
The password is held in memory only for the duration of the login request and
is discarded immediately after.

## 4. Third-party services

Verbinal does not integrate with any third-party services. The only external
communication is with the CANFAR platform as described above. CANFAR's own
privacy practices are governed by the
[CADC Terms of Use](https://www.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/en/about.html).

## 5. Children's privacy

Verbinal does not knowingly collect information from children under 13.
The app requires a CANFAR account, which is issued to researchers and
students by the Canadian Astronomy Data Centre.

## 6. Your choices

- **Don't save credentials:** Uncheck "Remember me" at login. No token or
  username will be persisted to the system keyring.
- **Clear saved credentials:** Log out from the app. This removes all stored
  tokens from the system keyring.
- **Clear recent launches:** Use the clear button in the Recent Launches panel.

## 7. Changes to this policy

If this policy changes, the updated version will be published in the
application's source repository. The effective date at the top of this document
will be updated accordingly.

## 8. Contact

If you have questions about this privacy policy:

- **GitHub:** [github.com/szautkin/CanfarDesktopUbuntu](https://github.com/szautkin/CanfarDesktopUbuntu) (open an issue)
- **Developer:** Serhii Zautkin

---

*This privacy policy applies to the Verbinal for Linux application distributed
via source code under the AGPL-3.0 license.*
