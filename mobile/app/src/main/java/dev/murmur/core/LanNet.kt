package dev.murmur.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.Socket

/**
 * Which Android network carries LAN traffic — Wi-Fi or ethernet,
 * never the VPN tunnel.
 *
 * Under an always-on VPN Android policy-routes every app socket into
 * the tunnel's fwmark table; with "block connections without VPN" the
 * tunnel table has no LAN route at all, so every outbound connect
 * dies EHOSTUNREACH while inbound keeps working — a mesh that can
 * only ever be dialed, never dial back. `Network.bindSocket` exists
 * for exactly this escape: it pins the socket to the LAN network,
 * outside VPN policy (same mechanism as the platform's lockdown-VPN
 * LAN exemption).
 */
object LanNet {
    fun lanNetwork(ctx: Context): android.net.Network? {
        val cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE)
            as ConnectivityManager
        return cm.allNetworks.firstOrNull { n ->
            val caps = cm.getNetworkCapabilities(n) ?: return@firstOrNull false
            (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
                caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) &&
                cm.getLinkProperties(n)?.linkAddresses?.any {
                    it.address.isSiteLocalAddress
                } == true
        }
    }

    /** `host` is a site-local LAN address — loopback (USB forwards)
     *  and public names keep the default routing. */
    private fun lanTarget(host: String): Boolean {
        val a = runCatching { java.net.InetAddress.getByName(host) }
            .getOrNull() ?: return false
        return a.isSiteLocalAddress || a.isLinkLocalAddress
    }

    fun bindSocket(ctx: Context, host: String, s: Socket) {
        if (!lanTarget(host)) return
        runCatching { lanNetwork(ctx)?.bindSocket(s) }
    }

    fun bindSocket(ctx: Context, s: DatagramSocket) {
        runCatching { lanNetwork(ctx)?.bindSocket(s) }
    }
}
