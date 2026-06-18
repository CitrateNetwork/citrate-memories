CREATE TABLE "conversations" (
	"id" text PRIMARY KEY NOT NULL,
	"owner" text NOT NULL,
	"org" text NOT NULL,
	"title" text DEFAULT 'Untitled' NOT NULL,
	"messages" jsonb DEFAULT '[]'::jsonb NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE INDEX "conversations_owner_org_idx" ON "conversations" USING btree ("owner","org");