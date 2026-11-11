#!/usr/bin/env bash
# Создать ключ подписи release-сборки: android/release.jks + android/keystore.properties.
# Оба файла в .gitignore: ключ подписи не публикуется. Потеряете ключ — обновления APK
# не установятся поверх, только удаление и установка заново.
set -euo pipefail
cd "$(dirname "$0")"
[ -f release.jks ] && { echo "release.jks уже есть"; exit 1; }
read -rsp "Пароль ключа (≥ 6 символов): " PW; echo
keytool -genkeypair -v -keystore release.jks -alias p2p-overlay -keyalg EC -groupname secp256r1 \
  -validity 10000 -storepass "$PW" -keypass "$PW" -dname "CN=P2P Overlay, O=Coursework"
cat > keystore.properties <<P
storeFile=release.jks
storePassword=$PW
keyAlias=p2p-overlay
keyPassword=$PW
P
chmod 600 keystore.properties release.jks
echo "Готово: ./gradlew assembleRelease подпишет APK этим ключом"
