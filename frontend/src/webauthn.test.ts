import { afterEach, describe, expect, it, vi } from 'vitest'
import { createPasskey, signWithPasskey } from './webauthn'

// "AQID" = bytes 1, 2, 3 in base64url
const bytes = (b: ArrayBuffer) => Array.from(new Uint8Array(b))
const buf = (...xs: number[]) => Uint8Array.from(xs).buffer

afterEach(() => vi.unstubAllGlobals())

describe('passkeys in the browser', () => {
  it('turns the server\'s options into buffers and the answer back into base64url', async () => {
    let seen: { publicKey: { challenge: ArrayBuffer; user: { id: ArrayBuffer }; excludeCredentials: { id: ArrayBuffer }[] } } | undefined
    vi.stubGlobal('navigator', {
      credentials: {
        create: async (o: never) => {
          seen = o
          return { id: 'cred', rawId: buf(255, 254), type: 'public-key', getClientExtensionResults: () => ({}),
            response: { attestationObject: buf(1), clientDataJSON: buf(2, 3) } }
        },
      },
    })
    const out = await createPasskey({ publicKey: { challenge: 'AQID', user: { id: 'BAU', name: 'a', displayName: 'A' },
      excludeCredentials: [{ id: 'Bg', type: 'public-key' }] } })
    expect(bytes(seen!.publicKey.challenge)).toEqual([1, 2, 3])
    expect(bytes(seen!.publicKey.user.id)).toEqual([4, 5])
    expect(bytes(seen!.publicKey.excludeCredentials[0].id)).toEqual([6])
    expect(out).toEqual({ id: 'cred', rawId: '__4', type: 'public-key', extensions: {},
      response: { attestationObject: 'AQ', clientDataJSON: 'AgM' } })
  })

  it('signs with an allowed credential', async () => {
    vi.stubGlobal('navigator', {
      credentials: {
        get: async () => ({ id: 'cred', rawId: buf(1), type: 'public-key', getClientExtensionResults: () => ({}),
          response: { authenticatorData: buf(1), clientDataJSON: buf(2), signature: buf(3), userHandle: null } }),
      },
    })
    const out = await signWithPasskey({ publicKey: { challenge: 'AQID', allowCredentials: [{ id: 'AQ', type: 'public-key' }] } })
    expect(out.response).toEqual({ authenticatorData: 'AQ', clientDataJSON: 'Ag', signature: 'Aw', userHandle: null })
  })
})
