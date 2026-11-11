package org.p2poverlay.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Секреты узла на устройстве.
 *  - KEK (32 байта) шифрует identity.key узла (узел получает его через P2P_IDENTITY_KEK и хранит
 *    ключ только как identity.key.enc). Сам KEK лежит в настройках зашифрованным ключом
 *    Android Keystore (аппаратное хранилище, ключ не извлекается из устройства).
 *  - Токен локального API (P2P_API_TOKEN): без него другие приложения телефона не могут
 *    обращаться к узлу на 127.0.0.1.
 */
object Secrets {
    private const val ALIAS = "p2p-overlay-kek-wrap"

    private fun wrapKey(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val g = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        g.init(KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .build())
        return g.generateKey()
    }

    private fun random(n: Int) = ByteArray(n).also { SecureRandom().nextBytes(it) }

    private fun hex(b: ByteArray) = b.joinToString("") { "%02x".format(it) }

    /** KEK для identity.key (hex). Создаётся один раз и хранится обёрнутым ключом Keystore. */
    fun identityKek(ctx: Context): String {
        val sp = ctx.getSharedPreferences("secrets", Context.MODE_PRIVATE)
        sp.getString("kek", null)?.let { stored ->
            val raw = Base64.decode(stored, Base64.NO_WRAP)
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.DECRYPT_MODE, wrapKey(), GCMParameterSpec(128, raw, 0, 12))
            return hex(c.doFinal(raw, 12, raw.size - 12))
        }
        val kek = random(32)
        val c = Cipher.getInstance("AES/GCM/NoPadding")
        c.init(Cipher.ENCRYPT_MODE, wrapKey())
        val blob = c.iv + c.doFinal(kek)
        sp.edit().putString("kek", Base64.encodeToString(blob, Base64.NO_WRAP)).commit()
        return hex(kek)
    }

    /** Токен локального API (постоянный для установки — узел и интерфейс в одном приложении). */
    fun apiToken(ctx: Context): String {
        val sp = ctx.getSharedPreferences("secrets", Context.MODE_PRIVATE)
        return sp.getString("api_token", null) ?: hex(random(24)).also { sp.edit().putString("api_token", it).commit() }
    }
}
