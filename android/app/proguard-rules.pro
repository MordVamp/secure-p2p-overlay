# Узел — отдельный процесс (libp2pnode.so), с приложением общается по HTTP/JSON: рефлексии нет.
# NodeService читает поле pid у java.lang.Process (класс платформы, R8 его не трогает).
# Номера строк в трассировках падений:
-keepattributes SourceFile,LineNumberTable
