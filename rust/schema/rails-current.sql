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

--
-- Name: pg_trgm; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;


SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: accounts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.accounts (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    join_code character varying,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: accounts_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.accounts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: accounts_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.accounts_id_seq OWNED BY public.accounts.id;


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
    config jsonb,
    created_at timestamp(6) without time zone NOT NULL,
    has_events boolean DEFAULT false NOT NULL,
    name character varying NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    sink_id bigint NOT NULL,
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
    created_at timestamp(6) without time zone NOT NULL,
    emoji character varying(8) DEFAULT '📌'::character varying NOT NULL,
    event_type character varying NOT NULL,
    idempotency_key character varying,
    occurred_at timestamp(6) without time zone NOT NULL,
    properties jsonb DEFAULT '{}'::jsonb NOT NULL,
    sdk_name character varying,
    sdk_version character varying,
    search_text text NOT NULL,
    sink_id bigint NOT NULL,
    text text NOT NULL,
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
-- Name: folders; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.folders (
    id bigint NOT NULL,
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    name character varying,
    updated_at timestamp(6) without time zone NOT NULL
);


--
-- Name: folders_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.folders_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: folders_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.folders_id_seq OWNED BY public.folders.id;


--
-- Name: saved_views; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.saved_views (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    event_date date,
    event_type character varying,
    name character varying NOT NULL,
    property_key character varying,
    property_op character varying,
    property_value character varying,
    search_query character varying,
    sink_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    user_id bigint NOT NULL
);


--
-- Name: saved_views_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.saved_views_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: saved_views_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.saved_views_id_seq OWNED BY public.saved_views.id;


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
    created_at timestamp(6) without time zone NOT NULL,
    ip_address character varying,
    updated_at timestamp(6) without time zone NOT NULL,
    user_agent character varying,
    user_id bigint NOT NULL
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
    column_last_viewed_at jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    has_unread_events boolean DEFAULT false NOT NULL,
    last_viewed_at timestamp(6) without time zone,
    sink_id bigint NOT NULL,
    unread_count integer DEFAULT 0 NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    user_id bigint NOT NULL
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
    account_id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    folder_id bigint,
    name character varying NOT NULL,
    token character varying,
    updated_at timestamp(6) without time zone NOT NULL
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
    account_id bigint,
    created_at timestamp(6) without time zone NOT NULL,
    current_folder_id bigint,
    current_sink_id bigint,
    email_address character varying NOT NULL,
    nickname character varying,
    password_digest character varying NOT NULL,
    role integer DEFAULT 0 NOT NULL,
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
-- Name: accounts id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.accounts ALTER COLUMN id SET DEFAULT nextval('public.accounts_id_seq'::regclass);


--
-- Name: columns id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.columns ALTER COLUMN id SET DEFAULT nextval('public.columns_id_seq'::regclass);


--
-- Name: events id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.events ALTER COLUMN id SET DEFAULT nextval('public.events_id_seq'::regclass);


--
-- Name: folders id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.folders ALTER COLUMN id SET DEFAULT nextval('public.folders_id_seq'::regclass);


--
-- Name: saved_views id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.saved_views ALTER COLUMN id SET DEFAULT nextval('public.saved_views_id_seq'::regclass);


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
-- Name: accounts accounts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_pkey PRIMARY KEY (id);


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
-- Name: folders folders_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.folders
    ADD CONSTRAINT folders_pkey PRIMARY KEY (id);


--
-- Name: saved_views saved_views_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.saved_views
    ADD CONSTRAINT saved_views_pkey PRIMARY KEY (id);


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
-- Name: index_accounts_on_join_code; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_accounts_on_join_code ON public.accounts USING btree (join_code);


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
-- Name: index_events_on_properties; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_events_on_properties ON public.events USING gin (properties);


--
-- Name: index_events_on_search_text_trgm; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_events_on_search_text_trgm ON public.events USING gin (search_text public.gin_trgm_ops);


--
-- Name: index_events_on_sink_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_events_on_sink_id ON public.events USING btree (sink_id);


--
-- Name: index_events_on_sink_id_and_idempotency_key; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_events_on_sink_id_and_idempotency_key ON public.events USING btree (sink_id, idempotency_key);


--
-- Name: index_events_on_sink_id_and_occurred_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_events_on_sink_id_and_occurred_at ON public.events USING btree (sink_id, occurred_at DESC);


--
-- Name: index_events_on_text_trgm; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_events_on_text_trgm ON public.events USING gin (text public.gin_trgm_ops);


--
-- Name: index_folders_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_folders_on_account_id ON public.folders USING btree (account_id);


--
-- Name: index_saved_views_on_sink_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_saved_views_on_sink_id ON public.saved_views USING btree (sink_id);


--
-- Name: index_saved_views_on_sink_id_and_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_saved_views_on_sink_id_and_user_id ON public.saved_views USING btree (sink_id, user_id);


--
-- Name: index_saved_views_on_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_saved_views_on_user_id ON public.saved_views USING btree (user_id);


--
-- Name: index_saved_views_on_user_id_and_sink_id_and_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_saved_views_on_user_id_and_sink_id_and_name ON public.saved_views USING btree (user_id, sink_id, name);


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
-- Name: index_sinks_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_sinks_on_account_id ON public.sinks USING btree (account_id);


--
-- Name: index_sinks_on_folder_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_sinks_on_folder_id ON public.sinks USING btree (folder_id);


--
-- Name: index_sinks_on_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_sinks_on_name ON public.sinks USING btree (name);


--
-- Name: index_sinks_on_token; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX index_sinks_on_token ON public.sinks USING btree (token);


--
-- Name: index_users_on_account_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_users_on_account_id ON public.users USING btree (account_id);


--
-- Name: index_users_on_current_folder_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_users_on_current_folder_id ON public.users USING btree (current_folder_id);


--
-- Name: index_users_on_current_sink_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX index_users_on_current_sink_id ON public.users USING btree (current_sink_id);


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
-- Name: users fk_rails_17b56651db; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT fk_rails_17b56651db FOREIGN KEY (current_folder_id) REFERENCES public.folders(id) ON DELETE SET NULL;


--
-- Name: users fk_rails_223f64897e; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT fk_rails_223f64897e FOREIGN KEY (current_sink_id) REFERENCES public.sinks(id) ON DELETE SET NULL;


--
-- Name: users fk_rails_61ac11da2b; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT fk_rails_61ac11da2b FOREIGN KEY (account_id) REFERENCES public.accounts(id);


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
-- Name: sinks fk_rails_bccb0938c9; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sinks
    ADD CONSTRAINT fk_rails_bccb0938c9 FOREIGN KEY (folder_id) REFERENCES public.folders(id);


--
-- Name: saved_views fk_rails_c8e04076d8; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.saved_views
    ADD CONSTRAINT fk_rails_c8e04076d8 FOREIGN KEY (sink_id) REFERENCES public.sinks(id);


--
-- Name: folders fk_rails_d2aa0af69f; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.folders
    ADD CONSTRAINT fk_rails_d2aa0af69f FOREIGN KEY (account_id) REFERENCES public.accounts(id);


--
-- Name: sinks fk_rails_e7d49e6417; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sinks
    ADD CONSTRAINT fk_rails_e7d49e6417 FOREIGN KEY (account_id) REFERENCES public.accounts(id);


--
-- Name: saved_views fk_rails_ecb864ffd2; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.saved_views
    ADD CONSTRAINT fk_rails_ecb864ffd2 FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: events fk_rails_fb783782a6; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.events
    ADD CONSTRAINT fk_rails_fb783782a6 FOREIGN KEY (sink_id) REFERENCES public.sinks(id);


--
-- PostgreSQL database dump complete
--
