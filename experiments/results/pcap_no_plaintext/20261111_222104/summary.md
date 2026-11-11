# pcap_no_plaintext

коммит `c732b144cdc0c8f545762b12837a8e36d499a21d` + незакоммиченные изменения (diff sha256 `cb3e3addf6d4`), N=15, k=3, α=3, R=3, relay=2, канал=tls, bootstrap=star, seed=1, конфиг `config/node_config.yaml`; драйвер `p2p-orchestrator scenario`

Захват `dumpcap` на lo, порты оверлея 7400–7414, во время 5 сообщений и 5 DHT STORE, содержащих маркер `PLAINTEXT-MARKER-5002f74da3b74849ace4f64e04b316dc`.

- маркер в трафике оверлея (overlay.pcapng, 550108 байт): **не найден**
- положительный контроль: тот же маркер в HTTP-запросах к панели узла (порты 7417–7431, открытый localhost-HTTP): найден — метод поиска работает
- протоколы в захвате оверлея — tshark_protocols.txt
