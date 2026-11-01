--
-- PostgreSQL database dump
--

-- Dumped from database version 16.8 (Homebrew)
-- Dumped by pg_dump version 16.8 (Homebrew)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: ar_internal_metadata; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ar_internal_metadata (
    key character varying NOT NULL,
    value character varying,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: columns; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.columns (
    id bigint NOT NULL,
    sink_id bigint NOT NULL,
    name character varying NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    config jsonb,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: columns_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.columns_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: columns_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.columns_id_seq OWNED BY public.columns.id;


--
-- Name: events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.events (
    id bigint NOT NULL,
    sink_id bigint NOT NULL,
    event_type character varying NOT NULL,
    text text,
    occurred_at timestamp(6) without time zone NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: events_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: events_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.events_id_seq OWNED BY public.events.id;


--
-- Name: schema_migrations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.schema_migrations (
    version character varying NOT NULL
);


--
-- Name: sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sessions (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    ip_address character varying,
    user_agent character varying,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: sessions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.sessions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: sessions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.sessions_id_seq OWNED BY public.sessions.id;


--
-- Name: sink_memberships; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sink_memberships (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    sink_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: sink_memberships_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.sink_memberships_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: sink_memberships_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.sink_memberships_id_seq OWNED BY public.sink_memberships.id;


--
-- Name: sinks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sinks (
    id bigint NOT NULL,
    name character varying NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    token character varying
);


--
-- Name: sinks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.sinks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: sinks_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.sinks_id_seq OWNED BY public.sinks.id;


--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users (
    id bigint NOT NULL,
    email_address character varying NOT NULL,
    password_digest character varying NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: users_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: users_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.users_id_seq OWNED BY public.users.id;


--
-- Name: columns id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.columns ALTER COLUMN id SET DEFAULT nextval('public.columns_id_seq'::regclass);


--
-- Name: events id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.events ALTER COLUMN id SET DEFAULT nextval('public.events_id_seq'::regclass);


--
-- Name: sessions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sessions ALTER COLUMN id SET DEFAULT nextval('public.sessions_id_seq'::regclass);


--
-- Name: sink_memberships id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sink_memberships ALTER COLUMN id SET DEFAULT nextval('public.sink_memberships_id_seq'::regclass);


--
-- Name: sinks id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sinks ALTER COLUMN id SET DEFAULT nextval('public.sinks_id_seq'::regclass);


--
-- Name: users id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users ALTER COLUMN id SET DEFAULT nextval('public.users_id_seq'::regclass);


--
-- Name: ar_internal_metadata ar_internal_metadata_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ar_internal_metadata
    ADD CONSTRAINT ar_internal_metadata_pkey PRIMARY KEY (key);


--
-- Name: columns columns_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.columns
    ADD CONSTRAINT columns_pkey PRIMARY KEY (id);


--
-- Name: events events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_pkey PRIMARY KEY (id);


--
-- Name: schema_migrations schema_migrations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.schema_migrations
    ADD CONSTRAINT schema_migrations_pkey PRIMARY KEY (version);


--
-- Name: sessions sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_pkey PRIMARY KEY (id);


--
-- Name: sink_memberships sink_memberships_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sink_memberships
    ADD CONSTRAINT sink_memberships_pkey PRIMARY KEY (id);


--
-- Name: sinks sinks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sinks
    ADD CONSTRAINT sinks_pkey PRIMARY KEY (id);


--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);


--
-- Name: index_columns_on_sink_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_columns_on_sink_id ON public.columns USING btree (sink_id);


--
-- Name: index_columns_on_sink_id_and_position; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_columns_on_sink_id_and_position ON public.columns USING btree (sink_id, "position");


--
-- Name: index_events_on_event_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_events_on_event_type ON public.events USING btree (event_type);


--
-- Name: index_events_on_payload; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_events_on_payload ON public.events USING gin (payload);


--
-- Name: index_events_on_sink_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_events_on_sink_id ON public.events USING btree (sink_id);


--
-- Name: index_events_on_sink_id_and_occurred_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_events_on_sink_id_and_occurred_at ON public.events USING btree (sink_id, occurred_at DESC);


--
-- Name: index_sessions_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_sessions_on_user_id ON public.sessions USING btree (user_id);


--
-- Name: index_sink_memberships_on_sink_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_sink_memberships_on_sink_id ON public.sink_memberships USING btree (sink_id);


--
-- Name: index_sink_memberships_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_sink_memberships_on_user_id ON public.sink_memberships USING btree (user_id);


--
-- Name: index_sink_memberships_on_user_id_and_sink_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_sink_memberships_on_user_id_and_sink_id ON public.sink_memberships USING btree (user_id, sink_id);


--
-- Name: index_sinks_on_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_sinks_on_name ON public.sinks USING btree (name);


--
-- Name: index_sinks_on_token; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_sinks_on_token ON public.sinks USING btree (token);


--
-- Name: index_users_on_email_address; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_users_on_email_address ON public.users USING btree (email_address);


--
-- Name: columns fk_rails_0cdb2abfef; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.columns
    ADD CONSTRAINT fk_rails_0cdb2abfef FOREIGN KEY (sink_id) REFERENCES public.sinks(id);


--
-- Name: sessions fk_rails_758836b4f0; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT fk_rails_758836b4f0 FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: sink_memberships fk_rails_9a1fa77e8d; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sink_memberships
    ADD CONSTRAINT fk_rails_9a1fa77e8d FOREIGN KEY (sink_id) REFERENCES public.sinks(id);


--
-- Name: sink_memberships fk_rails_9d46e8dc99; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sink_memberships
    ADD CONSTRAINT fk_rails_9d46e8dc99 FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: events fk_rails_fb783782a6; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.events
    ADD CONSTRAINT fk_rails_fb783782a6 FOREIGN KEY (sink_id) REFERENCES public.sinks(id);


--
-- PostgreSQL database dump complete
--


INSERT INTO public.schema_migrations(version) VALUES ('20260218070602'),('20260218070603'),('20260219111001'),('20260219111637'),('20260221041229'),('20260221041249'),('20260224101644');
