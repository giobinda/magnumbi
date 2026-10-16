// Passkeys in the browser: the server's options (binary values as base64url text) become the
// ArrayBuffers navigator.credentials wants, and the device's answer goes back as base64url JSON.

const toBuffer = (s: string): ArrayBuffer => {
  const b64 = s.replace(/-/g, '+').replace(/_/g, '/')
  const bin = atob(b64 + '='.repeat((4 - (b64.length % 4)) % 4))
  return Uint8Array.from(bin, c => c.charCodeAt(0)).buffer
}

const toText = (b: ArrayBuffer | null): string | null => {
  if (!b) return null
  let bin = ''
  for (const x of new Uint8Array(b)) bin += String.fromCharCode(x)
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

type Descriptor = { id: string; type: string; transports?: string[] }
type Options = { publicKey: Record<string, unknown> & { challenge: string } }

export const passkeysSupported = () => typeof window !== 'undefined' && 'PublicKeyCredential' in window

/** Create a passkey on this device (options from the server's register/start). */
export async function createPasskey(options: unknown) {
  const pk = (options as Options).publicKey
  const user = pk.user as { id: string; name: string; displayName: string }
  const publicKey = {
    ...pk,
    challenge: toBuffer(pk.challenge),
    user: { ...user, id: toBuffer(user.id) },
    excludeCredentials: ((pk.excludeCredentials as Descriptor[] | undefined) ?? []).map(c => ({ ...c, id: toBuffer(c.id) })),
  } as unknown as PublicKeyCredentialCreationOptions
  const cred = (await navigator.credentials.create({ publicKey })) as PublicKeyCredential | null
  if (!cred) throw new Error('No passkey was created')
  const r = cred.response as AuthenticatorAttestationResponse
  return {
    id: cred.id, rawId: toText(cred.rawId), type: cred.type, extensions: cred.getClientExtensionResults?.() ?? {},
    response: { attestationObject: toText(r.attestationObject), clientDataJSON: toText(r.clientDataJSON) },
  }
}

/** Sign with one of this account's passkeys (options from the server's login/start). */
export async function signWithPasskey(options: unknown) {
  const pk = (options as Options).publicKey
  const publicKey = {
    ...pk,
    challenge: toBuffer(pk.challenge),
    allowCredentials: ((pk.allowCredentials as Descriptor[] | undefined) ?? []).map(c => ({ ...c, id: toBuffer(c.id) })),
  } as unknown as PublicKeyCredentialRequestOptions
  const cred = (await navigator.credentials.get({ publicKey })) as PublicKeyCredential | null
  if (!cred) throw new Error('No passkey was used')
  const r = cred.response as AuthenticatorAssertionResponse
  return {
    id: cred.id, rawId: toText(cred.rawId), type: cred.type, extensions: cred.getClientExtensionResults?.() ?? {},
    response: {
      authenticatorData: toText(r.authenticatorData), clientDataJSON: toText(r.clientDataJSON),
      signature: toText(r.signature), userHandle: toText(r.userHandle),
    },
  }
}
