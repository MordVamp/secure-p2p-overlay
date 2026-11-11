package org.p2poverlay.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class InviteAndModesTest {
    private val id = "4078c0cfd3397ec2534e7240055aecf5a5066c6321a26ec2ca35053d07b469be"

    @Test fun parsesFullInvite() {
        val i = Invite.parse("p2p-overlay://192.168.10.237:7600?id=$id")!!
        assertEquals("192.168.10.237:7600", i.addr)
        assertEquals(id, i.nodeId)
    }

    @Test fun parsesAddressOnlyAndUppercaseId() {
        assertEquals("10.0.0.5:7000", Invite.parse("10.0.0.5:7000")!!.addr)
        assertEquals(id, Invite.parse("p2p-overlay://h.local:1?id=${id.uppercase()}")!!.nodeId)
    }

    @Test fun rejectsGarbage() {
        assertNull(Invite.parse("https://example.com"))
        assertNull(Invite.parse("p2p-overlay://host"))
        assertNull(Invite.parse("p2p-overlay://a b:12"))
        assertNull(Invite.parse("p2p-overlay://1.2.3.4:7600?id=zz")!!.nodeId)
    }

    @Test fun nodeIdCheck() {
        assertTrue(isNodeId(id))
        assertFalse(isNodeId(id.dropLast(1)))
        assertEquals("4078c0cf", shortId(id))
    }

    @Test fun ecoIsAlwaysClientOnly() {
        assertTrue("--no-relay" in PowerMode.ECO.nodeArgs(clientOnly = false))
        assertFalse("--no-relay" in PowerMode.NORMAL.nodeArgs(clientOnly = false))
        assertTrue("--no-relay" in PowerMode.MAX.nodeArgs(clientOnly = true))
    }

    @Test fun modesDifferInCost() {
        assertFalse(PowerMode.ECO.wakeLock)
        assertFalse(PowerMode.NORMAL.wakeLock)
        assertTrue(PowerMode.MAX.wakeLock)
        assertTrue(PowerMode.MAX.nodeArgs(false).containsAll(listOf("--pool-size", "2")))
        assertTrue(PowerMode.ECO.statusPollMs > PowerMode.MAX.statusPollMs)
        assertEquals(PowerMode.NORMAL, PowerMode.of("unknown"))
    }
}
