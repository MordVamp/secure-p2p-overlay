# pcap_no_plaintext

коммит `6726459277fb79ae3b5cfc0491d234fd48ac98a6` + незакоммиченные изменения (diff sha256 `fe4ceb1647cd`), N=15, k=3, α=3, R=3, bootstrap=star, seed=1; драйвер `p2p-orchestrator scenario`

Захват `dumpcap` на lo, порты оверлея 7400–7414, во время 5 сообщений и 5 DHT STORE, содержащих маркер `PLAINTEXT-MARKER-64a3ead7d16c44739540f8a6a98966f8`.

- маркер в трафике оверлея (overlay.pcapng, 599608 байт): **не найден**
- положительный контроль: тот же маркер в HTTP-запросах к панели узла (порты 7417–7431, открытый localhost-HTTP): найден — метод поиска работает
- протоколы в захвате оверлея — tshark_protocols.txt
