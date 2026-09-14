#include <core.p4>

struct ingress_metadata_t {
    bit<16> port;
    bool drop;
}

struct egress_metadata_t {
    bit<16> port;
    bool drop;
    bool broadcast;
}

/* The first GTS octet is reserved[7:4] followed by packet type[3:0]. */
header gts_kind_h {
    bit<4> reserved;
    bit<4> packet_type;
}

header gts_connect_h {
    bit<32> initiator_receive_tunnel;
    bit<32> initiator_reset_id;
    bit<16> profile;
    bit<8> initial_receive_credit;
}

header gts_connect_ack_h {
    bit<32> initiator_receive_tunnel;
    bit<32> responder_receive_tunnel;
    bit<32> responder_reset_id;
    bit<8> status;
    bit<8> initial_receive_credit;
    bit<8> reserved;
}

header gts_stream_open_h {
    bit<32> tunnel_id;
    bit<8> stream_id;
    bit<16> profile;
    bit<8> initial_receive_credit;
    bit<8> reserved;
}

header gts_stream_ack_h {
    bit<32> tunnel_id;
    bit<8> stream_id;
    bit<8> status;
    bit<8> initial_receive_credit;
    bit<8> reserved;
}

header gts_data_h {
    bit<32> tunnel_id;
    bit<8> stream_id;
    bit<32> sequence;
}

/* Always probe the next two bytes. For variable DATA and all DATA_END packets
 * this is valid_data_length. For fixed DATA these bytes belong to payload and
 * the common Rust semantic layer deliberately ignores the probe. */
header gts_data_option_h {
    bit<16> option_probe;
}

header gts_ack_h {
    bit<32> tunnel_id;
    bit<8> stream_id;
    bit<32> ack_base;
    bit<32> receive_bitmap;
    bit<8> receive_credit;
    bit<8> reserved;
}

header gts_datagram_h {
    bit<32> tunnel_id;
    bit<8> stream_id;
}

/* DATAGRAM has two independently optional fields: 32-bit sequence followed by
 * 16-bit valid-data length. The parser cannot consult a match-action profile
 * table before parsing, so it probes six bytes unconditionally. The common
 * semantic layer interprets 0, 2, 4, or 6 of them as metadata according to
 * the negotiated stream profile; any unused probe bytes remain payload. */
header gts_datagram_options_h {
    bit<8> option0;
    bit<8> option1;
    bit<8> option2;
    bit<8> option3;
    bit<8> option4;
    bit<8> option5;
}

header gts_stream_close_h {
    bit<32> tunnel_id;
    bit<8> stream_id;
    bit<32> final_sequence;
}

header gts_tunnel_close_h {
    bit<32> tunnel_id;
}

header gts_reset_h {
    bit<32> tunnel_id;
    bit<32> reset_id;
    bit<8> reason;
}

header gts_stream_reset_h {
    bit<32> tunnel_id;
    bit<8> stream_id;
    bit<8> reason;
    bit<8> reserved;
}

struct headers_t {
    gts_kind_h kind;
    gts_connect_h connect;
    gts_connect_ack_h connect_ack;
    gts_stream_open_h stream_open;
    gts_stream_ack_h stream_ack;
    gts_data_h data;
    gts_data_option_h data_option;
    gts_ack_h ack;
    gts_datagram_h datagram;
    gts_datagram_options_h datagram_options;
    gts_stream_close_h stream_close;
    gts_tunnel_close_h tunnel_close;
    gts_reset_h reset;
    gts_stream_reset_h stream_reset;
}

parser parse(
    packet_in pkt,
    out headers_t hdr,
    inout ingress_metadata_t ingress,
) {
    state start {
        pkt.extract(hdr.kind);
        if (hdr.kind.reserved != 4w0) {
            transition reject;
        }
        if (hdr.kind.packet_type == 4w1) {
            transition parse_connect;
        }
        if (hdr.kind.packet_type == 4w2) {
            transition parse_connect_ack;
        }
        if (hdr.kind.packet_type == 4w3) {
            transition parse_stream_open;
        }
        if (hdr.kind.packet_type == 4w4) {
            transition parse_stream_ack;
        }
        if (hdr.kind.packet_type == 4w5) {
            transition parse_data;
        }
        if (hdr.kind.packet_type == 4w6) {
            transition parse_ack;
        }
        if (hdr.kind.packet_type == 4w7) {
            transition parse_data;
        }
        if (hdr.kind.packet_type == 4w8) {
            transition parse_stream_close;
        }
        if (hdr.kind.packet_type == 4w9) {
            transition parse_stream_close;
        }
        if (hdr.kind.packet_type == 4w10) {
            transition parse_tunnel_close;
        }
        if (hdr.kind.packet_type == 4w11) {
            transition parse_tunnel_close;
        }
        if (hdr.kind.packet_type == 4w12) {
            transition parse_reset;
        }
        if (hdr.kind.packet_type == 4w13) {
            transition parse_datagram;
        }
        if (hdr.kind.packet_type == 4w14) {
            transition parse_stream_reset;
        }
        if (hdr.kind.packet_type == 4w15) {
            transition parse_stream_reset;
        }
        transition reject;
    }

    state parse_connect {
        pkt.extract(hdr.connect);
        transition accept;
    }

    state parse_connect_ack {
        pkt.extract(hdr.connect_ack);
        transition accept;
    }

    state parse_stream_open {
        pkt.extract(hdr.stream_open);
        transition accept;
    }

    state parse_stream_ack {
        pkt.extract(hdr.stream_ack);
        transition accept;
    }

    state parse_data {
        pkt.extract(hdr.data);
        pkt.extract(hdr.data_option);
        transition accept;
    }

    state parse_ack {
        pkt.extract(hdr.ack);
        transition accept;
    }

    state parse_datagram {
        pkt.extract(hdr.datagram);
        pkt.extract(hdr.datagram_options);
        transition accept;
    }

    state parse_stream_close {
        pkt.extract(hdr.stream_close);
        transition accept;
    }

    state parse_tunnel_close {
        pkt.extract(hdr.tunnel_close);
        transition accept;
    }

    state parse_reset {
        pkt.extract(hdr.reset);
        transition accept;
    }

    state parse_stream_reset {
        pkt.extract(hdr.stream_reset);
        transition accept;
    }
}

control ingress(
    inout headers_t hdr,
    inout ingress_metadata_t ingress,
    inout egress_metadata_t egress,
) {
    apply {
        egress.port = ingress.port;
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
