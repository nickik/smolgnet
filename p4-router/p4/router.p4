#include <core.p4>

struct ingress_metadata_t {
    bit<16> port;
    bool drop;
}

struct egress_metadata_t {
    bit<16> port;
    bool drop;
    bool broadcast;
    bool transit;
    bool decided;
}

header gdp_base_h {
    bit<2> version;
    bit<4> packet_type;
    bit<4> size_class;
    bit<1> local_form;
    bit<5> reserved;
    bit<8> crc8;
    bit<8> hop;
}

header gdp_global_h {
    bit<32> destination_hi;
    bit<32> destination_lo;
    bit<32> source_hi;
    bit<32> source_lo;
}

header gdp_local_h {
    bit<16> destination;
    bit<16> source;
}

struct headers_t {
    gdp_base_h base;
    gdp_global_h global_addr;
    gdp_local_h local_addr;
}

parser parse(
    packet_in pkt,
    out headers_t hdr,
    inout ingress_metadata_t ingress,
) {
    state start {
        pkt.extract(hdr.base);
        if (hdr.base.local_form == 1w1) {
            transition parse_local;
        }
        transition parse_global;
    }

    state parse_global {
        pkt.extract(hdr.global_addr);
        transition accept;
    }

    state parse_local {
        pkt.extract(hdr.local_addr);
        transition accept;
    }
}

control ingress(
    inout headers_t hdr,
    inout ingress_metadata_t ingress,
    inout egress_metadata_t egress,
) {
    action allow() {
    }

    action drop() {
        egress.drop = true;
        egress.decided = true;
        egress.transit = false;
    }

    action punt(bit<16> cpu_port) {
        egress.port = cpu_port;
        egress.decided = true;
        egress.transit = false;
    }

    action deliver_local(bit<16> port) {
        egress.port = port;
        egress.decided = true;
        egress.transit = false;
    }

    action forward(bit<16> port) {
        egress.port = port;
        egress.decided = true;
        egress.transit = true;
    }

    /* Per-ingress policy executes before routing. */
    table policy {
        key = {
            ingress.port: exact;
            hdr.base.packet_type: exact;
        }
        actions = {
            allow;
            drop;
            punt;
        }
        default_action = allow;
        size = 256;
    }

    /*
     * GDP global addresses are represented as two 32-bit fields for stable
     * x4c code generation. Prefixes /33..64 use exact high32 + LPM low32.
     */
    table global_long_routes {
        key = {
            hdr.global_addr.destination_hi: exact;
            hdr.global_addr.destination_lo: lpm;
        }
        actions = {
            drop;
            punt;
            deliver_local;
            forward;
        }
        default_action = allow;
        size = 4096;
    }

    /* Prefixes /0..32 use LPM on the high 32 bits. */
    table global_short_routes {
        key = {
            hdr.global_addr.destination_hi: lpm;
        }
        actions = {
            drop;
            punt;
            deliver_local;
            forward;
        }
        default_action = drop;
        size = 4096;
    }

    /* Compact local GDP is never transit-routed in v0.1. */
    table local_routes {
        key = {
            hdr.local_addr.destination: exact;
        }
        actions = {
            drop;
            punt;
            deliver_local;
        }
        default_action = drop;
        size = 1024;
    }

    apply {
        egress.decided = false;
        egress.transit = false;

        policy.apply();

        if (!egress.decided) {
            if (hdr.global_addr.isValid()) {
                global_long_routes.apply();
                if (!egress.decided) {
                    global_short_routes.apply();
                }
            } else {
                local_routes.apply();
            }
        }

        /* GDP CRC-8 excludes hop limit, so transit decrement needs no CRC fixup. */
        if (egress.transit) {
            if (hdr.base.hop == 8w0) {
                egress.drop = true;
            } else {
                if (hdr.base.hop == 8w1) {
                    egress.drop = true;
                } else {
                    hdr.base.hop = hdr.base.hop - 8w1;
                }
            }
        }
    }
}

control egress(
    inout headers_t hdr,
    inout ingress_metadata_t ingress,
    inout egress_metadata_t egress,
) {
}

SoftNPU(
    parse(),
    ingress(),
    egress()
) main;
